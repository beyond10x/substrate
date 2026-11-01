//! Design 34 § 10.2 steps 3 to 7 of `cargo xtask namespace-lane observe` (§ 12 U13): the daemon
//! with `--driver kubernetes` on the workstation, the vectors' namespace lane, the driver-specific
//! cases of ADR 0047 decision 13 and the cleanup assertions.
//!
//! Every daemon the lane starts is the shipped binary, built from this checkout; the lane talks to
//! it over its Unix socket and to the cluster with the admin credential only to set a case up, to
//! look at what the driver left and to release the namespace's Lease once the daemon that held it
//! is stopped: the Role grants the driver no `delete` on Leases (decision 3), so a stopped
//! daemon's claim would hold the namespace for 30 s, or for ever for another deployment.
//!
//! Step 4 runs design 31's clean-room runner, built with `-p` beside the daemon, against the
//! lane's daemon served over TLS as well, with the chart lane's Identity stub on loopback
//! (`xtask/src/chart_cluster.rs`); its restart hook kills the daemon, and the step starts it again
//! on the same state, certificate and port. A case the driver cannot reach on this wave is named
//! in [`ABSENT`] with its reason; every other case in [`cases`] must pass once.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use substrate_kubernetes_client::{Api, ListForm};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;

use crate::chart_cluster::{Listener, Material, clean_room, output};
use crate::namespace_lane_render as render;

/// The driver's kubeconfig context extension (`crates/substrate-kubernetes/src/lib.rs`).
pub const EXTENSION: &str = "b10x-substrate/namespace-driver";
/// The coordination Lease the driver claims its namespace with.
pub const LEASE: &str = "b10x-substrate-driver";
/// The deployment of the lane's own daemon (steps 3, 6 and 7).
pub const LANE_DEPLOYMENT: &str = "dep_namespace_lane";
/// The deployment the vectors' namespace lane serves; `runtime_vectors.rs` names the same one.
pub const VECTORS_DEPLOYMENT: &str = "dep_namespace_vectors";
/// The deployment of the Role-verb cases.
const VERBS_DEPLOYMENT: &str = "dep_namespace_verbs";
/// The deployment of the privileged dry-run case, in the unlabelled namespace.
const UNLABELLED_DEPLOYMENT: &str = "dep_namespace_unlabelled";
/// What selects the vectors' namespace lane: the driver's kubeconfig.
pub const KUBERNETES_CONFIG_VARIABLE: &str = "SUBSTRATE_VECTORS_KUBERNETES_CONFIG";
/// The vectors' namespace lane, by its test name.
pub const VECTORS_TEST: &str =
    "namespace_lane_drives_the_shipped_daemon_over_the_kubernetes_driver";
/// The source that pins `NAMESPACE_CASES`.
pub const VECTORS_SOURCE: &str = "crates/substrate-daemon/tests/runtime_vectors.rs";
/// The line the vectors' namespace lane prints once its count held.
pub const VECTORS_LINE: &str = "runtime clean-room: namespace lane executed ";
/// The package the daemon and its vectors are in.
const DAEMON_PACKAGE: &str = "b10x-substrate-daemon";

/// The probe period every lane daemon states: one probe at open, none during the cases.
const PROBE_INTERVAL_SECONDS: u64 = 3_600;
/// What a daemon may take from start to a published, probed snapshot: Lease, policy, dry run,
/// a PVC bound by first consumer, a keeper and a probe Pod, each image pulled once.
const SERVING_BOUND: Duration = Duration::from_mins(4);
/// What a workspace destroy left to the daemon's reconciliation may take: its backoff reaches
/// 30 s, and PVC protection waits for the keeper.
const ABSENCE_BOUND: Duration = Duration::from_mins(3);
/// How long a Role-verb case watches for its dependent to be refused: a probe on pulled images,
/// then past the Lease's 20 s held window and one more 10 s round, so a lost renewal shows.
const VERB_BOUND: Duration = Duration::from_mins(2);
/// How long a Role-verb case waits on a pending destroy to finish or name why it cannot: the
/// keeper's termination comes first, and a loaded kind node took minutes to kill a Pod (run 2,
/// 2026-10-29, where 90 s ran out before the PVC's deletion was tried).
const PENDING_BOUND: Duration = Duration::from_mins(4);
/// The daemon's answer to a mutation whose driver reported progress, not an end.
const OUTCOME_UNKNOWN: &str = "operation.outcome-unknown";
/// Where, under the checkout, each run keeps the daemon logs of its failed cases.
const KEPT_LOGS: &str = "target/namespace-lane/daemon-logs";
/// How far a file's modification time may lag the clock a case's start was read from: the kernel
/// stamps files from its coarse clock, at most one tick (10 ms at 100 Hz) behind. Verb cases run
/// back to back, so the previous case's log is older than its start by more than this.
const MTIME_SLACK: Duration = Duration::from_millis(20);
/// When a Role-verb case whose facts stayed whole tries its dependent route.
const VERB_ROUTE_AFTER: Duration = Duration::from_secs(5);
/// How long the privileged dry-run case waits for its refusal.
const REFUSAL_BOUND: Duration = Duration::from_secs(90);
/// How long the cluster-scoped inventory may take to settle back: dynamic PVs go after their
/// claims.
const INVENTORY_BOUND: Duration = Duration::from_mins(3);

/// The name step 4's daemon certificate carries; the runner verifies it apart from the loopback
/// address it dials.
const RUNNER_SERVER_NAME: &str = "namespace-lane.substrate.test";
/// A name step 4's daemon certificate does not carry, under which the runner must fail to verify.
const RUNNER_FORBIDDEN_NAME: &str = "headless.namespace-lane.substrate.test";
/// Step 4's restart hook, run argv-only by the runner as `kill -KILL <daemon pid>`: a crash, not
/// the graceful path (design 31 § 7).
const KILL_PROGRAM: &str = "/usr/bin/kill";
/// The runner's own deadline in step 4: each workspace is a claim and a keeper Pod on kind.
const RUNNER_DEADLINE: Duration = Duration::from_mins(30);
/// The runner's bound on one call, its ceiling.
const RUNNER_IO: Duration = Duration::from_mins(1);
/// The runner's bound on the hook and the new probe: the kill, the Lease, [`SERVING_BOUND`].
const RUNNER_RESTART: Duration = Duration::from_mins(10);
/// The input step 4 leaves undeclared: the Identity stub resolves one credential and narrows
/// none, so the provider answers `unsupported` for the narrowed purpose.
const RUNNER_UNDECLARED: [&str; 1] = ["narrowed-credential"];
/// Why every exec-dependent case is absent on this wave.
pub const NO_EXEC: &str = "no exec is admitted on this wave: the driver never publishes \
                           confinement.no-egress (O3 did not hold on kind), so every exec start \
                           is refused exec.sandbox-unavailable before a Pod exists";

/// The cases named absent, with their reasons; none is ever counted as passed.
pub const ABSENT: [(&str, &str); 6] = [
    ("S6 quota exec.capacity", NO_EXEC),
    (
        "S6 restart: running exec reads unknown and its Pod is gone",
        NO_EXEC,
    ),
    ("S6 force-deleted Pod is not reported ended early", NO_EXEC),
    (
        "S6 Role without pods watch",
        "the driver watches Pods only on the exec path (D173), and no exec is admitted on this \
         wave: the driver never publishes confinement.no-egress",
    ),
    (
        "S6 Role without persistentvolumeclaims list",
        "no driver path lists PersistentVolumeClaims, so the verb has no dependent fact or route \
         to refuse: decision 3 grants a verb the driver does not use",
    ),
    (
        "S6 Role without networkpolicies get",
        "the driver reads NetworkPolicies only by list (the default-deny check); no driver path \
         gets one, so the verb has no dependent fact or route to refuse: decision 3 grants a verb \
         the driver does not use",
    ),
];

/// Every case of steps 3 to 7, in the order they run: the fixed ones and one per Role verb.
pub fn cases() -> Vec<String> {
    let mut names: Vec<String> = [
        "S3 daemon serves with --driver kubernetes",
        "S4 clean-room runner",
        "S5 runtime vectors, namespace lane",
        "S6 same-named object of another deployment untouched",
        "S6 foreign-uid object untouched",
        "S6 quota workspace.capacity",
        "S6 quota exec.capacity",
        "S6 privileged dry run refuses startup",
        "S6 restart: running exec reads unknown and its Pod is gone",
        "S6 force-deleted Pod is not reported ended early",
    ]
    .map(str::to_owned)
    .to_vec();
    names.extend(
        role_verbs()
            .into_iter()
            .map(|(resource, verb)| verb_case(&resource, &verb)),
    );
    names.push("S7 everything destroyed through the daemon".to_owned());
    names
}

/// The name of the case that removes `verb` on `resource` from the Role.
pub fn verb_case(resource: &str, verb: &str) -> String {
    format!("S6 Role without {resource} {verb}")
}

/// Every `(resource, verb)` of decision 3's Role, in its order.
pub fn role_verbs() -> Vec<(String, String)> {
    render::role_grants()
        .into_iter()
        .flat_map(|grant| {
            grant
                .verbs
                .into_iter()
                .map(move |verb| (grant.resource.clone(), verb))
        })
        .collect()
}

/// What one case showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Case {
    Passed(String),
    Absent(String),
    Failed(String),
}

/// Why the cases are no record, or `None` when every case of [`cases`] ran once and passed or is
/// named in [`ABSENT`] and reported absent with that reason.
pub fn verdict(outcomes: &[(String, Case)]) -> Option<String> {
    let mut faults = Vec::new();
    for name in cases() {
        let absent = ABSENT
            .iter()
            .find(|(absent, _)| *absent == name)
            .map(|(_, reason)| *reason);
        let reported: Vec<&Case> = outcomes
            .iter()
            .filter(|(reported, _)| *reported == name)
            .map(|(_, case)| case)
            .collect();
        match (reported.as_slice(), absent) {
            ([Case::Passed(_)], None) => {}
            ([Case::Absent(reason)], Some(named)) if reason == named => {}
            ([Case::Failed(error)], _) => faults.push(format!("{name} failed: {error}")),
            ([], _) => faults.push(format!("{name} did not run")),
            ([_], None) => faults.push(format!("{name} is not declared absent")),
            ([_], Some(_)) => {
                faults.push(format!("{name} is declared absent and must be reported so"));
            }
            _ => faults.push(format!("{name} ran more than once")),
        }
    }
    let declared = cases();
    for (name, _) in outcomes {
        if !declared.contains(name) {
            faults.push(format!("{name} is not a declared case"));
        }
    }
    (!faults.is_empty()).then(|| faults.join("; "))
}

/// The images every lane daemon names, and where its no-egress probe would dial.
#[derive(Debug, Clone)]
pub struct Settings {
    pub executor_image: String,
    pub exec_image: String,
    pub egress_probe_address: String,
}

/// `driver` (a JSON driver kubeconfig) with decision-free extension `settings` on its current
/// context: the lane's storage class and claim size, the kubelet's stated bounds, and one probe
/// per hour.
pub fn with_extension(mut driver: Value, settings: &Settings) -> Value {
    let extension = json!({
        "storage_class": render::STORAGE_CLASS,
        "workspace_size_bytes": 64 * 1_048_576_u64,
        "executor_image": settings.executor_image,
        "exec_image": settings.exec_image,
        "default_deny_policy": render::DEFAULT_DENY,
        "egress_probe_address": settings.egress_probe_address,
        "pod_pids_limit": 1_024,
        "container_log_max_bytes": 10 * 1_048_576_u64,
        "run_as_user": render::RUN_AS_USER,
        "probe_interval_seconds": PROBE_INTERVAL_SECONDS,
    });
    if let Some(contexts) = driver["contexts"].as_array_mut() {
        for context in contexts {
            context["context"]["extensions"] = json!([{"name": EXTENSION, "extension": extension}]);
        }
    }
    driver
}

/// The `NAMESPACE_CASES` the source pins, or `None` when it is not declared in the one form.
pub fn declared_namespace_cases(source: &str) -> Option<usize> {
    source.lines().find_map(|line| {
        line.strip_prefix("const NAMESPACE_CASES: usize = ")?
            .strip_suffix(';')?
            .parse()
            .ok()
    })
}

/// Whether the vectors' output shows the namespace lane ran as the one selected case and executed
/// exactly `declared` cases; the count it printed.
///
/// # Errors
///
/// The harness did not report `1 passed`, or the lane printed no count or another one.
pub fn vectors_verdict(stdout: &str, declared: usize) -> Result<usize, String> {
    if !stdout
        .lines()
        .any(|line| line.starts_with("test result: ok. 1 passed;"))
    {
        return Err(format!(
            "{VECTORS_TEST} selected no case or did not pass; the harness reported no `1 passed`"
        ));
    }
    let printed: Vec<usize> = stdout
        .lines()
        .filter_map(|line| {
            line.strip_prefix(VECTORS_LINE)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .collect();
    match printed.as_slice() {
        [count] if *count == declared => Ok(*count),
        [count] => Err(format!(
            "the namespace lane executed {count} cases where NAMESPACE_CASES is {declared}"
        )),
        _ => Err(format!(
            "the namespace lane printed {} case counts, not one",
            printed.len()
        )),
    }
}

/// The driver's probe-loop lines that a probe has finished, run or failed: before one, a fact
/// may be absent only because no probe ran yet.
pub const PROBE_FINISHED: [&str; 2] = [
    "the namespace driver's probe ran",
    "the namespace driver's probe failed",
];

/// The driver's line that its open was refused: no probe runs after it, and it carries the API's
/// answer as `message=`.
pub const NOT_OPEN: &str = "the namespace driver is not open";

/// The reason codes a daemon's log names on its `WARN` lines: `code=` fields, quoted or not.
pub fn named_codes(log: &str) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for line in log.lines().filter(|line| line.contains("WARN")) {
        let plain = without_escapes(line);
        let mut rest = plain.as_str();
        while let Some(at) = rest.find("code=") {
            rest = &rest[at + "code=".len()..];
            let value: String = rest
                .trim_start_matches('"')
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
                })
                .collect();
            if !value.is_empty() {
                codes.insert(value);
            }
        }
    }
    codes
}

/// The reason codes the daemon named for a pending destroy of `workspace`: the `code=` fields of
/// `WARN` lines written after the first `since` bytes of its log that carry `workspace=<id>`, as
/// the destroy's first answer and every reconciliation round write them. A code is counted each
/// time it is written, so one the log already held, such as the probe's own teardown naming
/// `kubernetes.api-failed` for its PVC, does not hide the destroy's.
pub fn pending_destroy_codes(log: &str, since: usize, workspace: &str) -> BTreeSet<String> {
    let after = String::from_utf8_lossy(&log.as_bytes()[since.min(log.len())..]).into_owned();
    let field = format!("workspace={workspace}");
    let lines: Vec<String> = after
        .lines()
        .filter(|line| {
            without_escapes(line)
                .split_whitespace()
                .any(|word| word == field)
        })
        .map(str::to_owned)
        .collect();
    named_codes(&lines.join("\n"))
}

/// The facts a daemon's log says, on a `WARN` line, were `<fact> was not proved`: the probe's
/// own statement for `exec.argv-only`, which carries the API's answer as `why=` and no code.
pub fn unproved_facts(log: &str) -> BTreeSet<String> {
    const NOT_PROVED: &str = " was not proved";
    log.lines()
        .filter(|line| line.contains("WARN"))
        .filter_map(|line| {
            let plain = without_escapes(line);
            let before = &plain[..plain.find(NOT_PROVED)?];
            let fact = before.rsplit(char::is_whitespace).next()?;
            (!fact.is_empty()).then(|| fact.to_owned())
        })
        .collect()
}

/// The API server's answer refusing `verb` on `resource`, in its own words. The closing quote
/// keeps `pods` from matching `pods/log` or `pods/exec`.
pub fn api_refusal(resource: &str, verb: &str) -> String {
    format!("cannot {verb} resource \"{resource}\"")
}

/// A Role-verb case's refusal once a full-set fact is `missing`: ADR 0047 decision 13's "the
/// dependent fact is absent and its refusal named". Only the log after its first `since` bytes
/// counts (the lane starts each verb's daemon on a fresh log after the Role change, so `0`), and
/// in it a `WARN` line must carry the API's answer refusing exactly `verb` on `resource`
/// ([`api_refusal`]), in the plain form or the `Debug`-escaped one; for `pods/exec` `create`
/// alone, the API's bare refusal of the exec counts too once the driver's access review names
/// `create` as the verb it denies ([`bare_exec_refusal`]), since the API gives that refusal no
/// reason (D349, `story:driver-names-a-bare-exec-refusals-verb`). The case decides only once
/// the first probe has finished ([`PROBE_FINISHED`]) or the refusal is the open's own
/// ([`NOT_OPEN`]), after which no probe runs: never on facts absent because no probe ran yet. A
/// reason code or a fact's "was not proved" line alone names no resource and refuses nothing.
pub fn verb_refusal(
    log: &str,
    since: usize,
    resource: &str,
    verb: &str,
    missing: &BTreeSet<String>,
) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    let after = String::from_utf8_lossy(&log.as_bytes()[since.min(log.len())..]).into_owned();
    // Resources and verbs hold no backslash, so dropping every one reads `\"` as `"`.
    let lines: Vec<String> = after
        .lines()
        .map(|line| without_escapes(line).replace('\\', ""))
        .collect();
    let bare = resource == "pods/exec" && verb == "create";
    let answer = if bare {
        format!(
            "{} or a bare exec refusal {REVIEW_DENIES_CREATE}",
            api_refusal(resource, verb)
        )
    } else {
        api_refusal(resource, verb)
    };
    let refused: Vec<&String> = lines
        .iter()
        .filter(|line| {
            line.contains("WARN")
                && (line.contains(&api_refusal(resource, verb))
                    || (bare && bare_exec_refusal(line)))
        })
        .collect();
    let finished = lines
        .iter()
        .any(|line| PROBE_FINISHED.iter().any(|said| line.contains(said)))
        || refused.iter().any(|line| line.contains(NOT_OPEN));
    let first = refused.first().filter(|_| finished)?;
    let unproved: Vec<String> = missing
        .intersection(&unproved_facts(&after))
        .cloned()
        .collect();
    Some(format!(
        "absent {missing:?}; after the first probe the log carries the API's answer `{answer}` \
         on: {}; not proved {unproved:?}",
        first.trim()
    ))
}

/// What the driver writes after the API's bare refusal of an exec once its
/// `SelfSubjectAccessReview` denies `pods/exec` `create` alone (`ACCESS_REVIEW_DENIES` in
/// `crates/substrate-kubernetes/src/namespace/workspace.rs`, design 34 § 7.1).
const REVIEW_DENIES_CREATE: &str = "(a SelfSubjectAccessReview denies pods/exec create)";

/// Whether `line` is the executor handshake refused 403 for the probe's keeper Pod with no reason
/// from the API, and the driver's access review naming `create` alone as the verb it denies: how
/// the API (kind v1.37, D349) refuses an exec upgrade the Role lacks `pods/exec` `create` for, and
/// what the driver adds to it (`story:driver-names-a-bare-exec-refusals-verb`). The message names
/// the keeper (`pods "sbx-pr-…-keeper" is forbidden:`) and nothing follows it but
/// [`REVIEW_DENIES_CREATE`]. A bare refusal whose review names no verb, denies `get` or both, did
/// not decide or failed is not the create case's refusal, and neither is a line carrying any
/// other reason.
pub fn bare_exec_refusal(line: &str) -> bool {
    const ANSWERED: &str = "exec answered 403: pods \"";
    const FORBIDDEN: &str = "\" is forbidden:";
    if !line.contains("code=\"kubernetes.executor-handshake-failed\"") {
        return false;
    }
    let Some(start) = line.find(ANSWERED) else {
        return false;
    };
    let rest = &line[start + ANSWERED.len()..];
    let Some(end) = rest.find(FORBIDDEN) else {
        return false;
    };
    let pod = &rest[..end];
    let reason = rest[end + FORBIDDEN.len()..].trim().trim_end_matches('"');
    pod.starts_with("sbx-pr-")
        && pod.ends_with("-keeper")
        && !pod.contains(['"', ' '])
        && reason.trim() == REVIEW_DENIES_CREATE
}

/// `line` without its ANSI CSI sequences (`ESC [ … final byte`), should the log be coloured.
fn without_escapes(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            for inner in characters.by_ref() {
                if ('@'..='~').contains(&inner) && inner != '[' {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

/// The names of the facts a `/v1/machine` result publishes (present and not null).
pub fn published(machine: &Value) -> BTreeSet<String> {
    machine["facts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, _)| name.clone())
        .collect()
}

/// The labelled objects step 7 found, as `kind/name`, beyond the one each namespace may keep:
/// the Lease, which the Role cannot delete (decision 3) and the lane releases as admin.
pub fn unexpected_leftovers(found: &[String]) -> Vec<String> {
    found
        .iter()
        .filter(|object| *object != &format!("Lease/{LEASE}"))
        .cloned()
        .collect()
}

/// Cluster-scoped names by list kind.
pub type Inventory = BTreeMap<String, BTreeSet<String>>;

/// The difference between two inventories, empty when equal.
pub fn inventory_difference(before: &Inventory, after: &Inventory) -> Vec<String> {
    let kinds: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut differences = Vec::new();
    let empty = BTreeSet::new();
    for kind in kinds {
        let was = before.get(kind).unwrap_or(&empty);
        let is = after.get(kind).unwrap_or(&empty);
        for added in is.difference(was) {
            differences.push(format!("{kind} gained {added}"));
        }
        for removed in was.difference(is) {
            differences.push(format!("{kind} lost {removed}"));
        }
    }
    differences
}

/// The namespaced collections step 7 searches for the managed-by label, as `(path, list kind)`.
fn labelled_collections(namespace: &str) -> Vec<(String, &'static str, &'static str)> {
    vec![
        (
            format!("/api/v1/namespaces/{namespace}/pods"),
            "PodList",
            "Pod",
        ),
        (
            format!("/api/v1/namespaces/{namespace}/persistentvolumeclaims"),
            "PersistentVolumeClaimList",
            "PersistentVolumeClaim",
        ),
        (
            format!("/apis/coordination.k8s.io/v1/namespaces/{namespace}/leases"),
            "LeaseList",
            "Lease",
        ),
        (
            format!("/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies"),
            "NetworkPolicyList",
            "NetworkPolicy",
        ),
        (
            format!("/api/v1/namespaces/{namespace}/configmaps"),
            "ConfigMapList",
            "ConfigMap",
        ),
        (
            format!("/api/v1/namespaces/{namespace}/secrets"),
            "SecretList",
            "Secret",
        ),
        (
            format!("/api/v1/namespaces/{namespace}/services"),
            "ServiceList",
            "Service",
        ),
        (
            format!("/api/v1/namespaces/{namespace}/serviceaccounts"),
            "ServiceAccountList",
            "ServiceAccount",
        ),
    ]
}

/// What shows a started daemon is ready for the lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    /// Its Unix socket exists, within 30 s.
    Socket,
    /// `GET /readyz` on this operational listener answers 200, within [`SERVING_BOUND`]. A daemon
    /// given `--tls-listen` serves TLS instead of its Unix socket and never creates one; its
    /// `/readyz` answers 200 only once the listener accepts and the driver's opening settled.
    Ops(SocketAddr),
}

/// One daemon the lane started: the shipped binary on a Unix socket, its stderr in a file.
pub struct Daemon {
    child: Option<Child>,
    socket: PathBuf,
    log: PathBuf,
    sequence: u64,
}

impl Daemon {
    /// Starts `binary` with `--driver kubernetes`, `config` and `deployment`, its state and socket
    /// in `directory` (created owner-private), and waits for its socket.
    ///
    /// # Errors
    ///
    /// The process does not start, or exits or creates no socket within 30 s.
    pub fn start(binary: &Path, directory: &Path, config: &Path, deployment: &str) -> Result<Self> {
        Self::start_with(binary, directory, config, deployment, &[], Ready::Socket)
    }

    /// [`Daemon::start`] with `extra` arguments after the lane's own (step 4's TLS listener),
    /// waiting for what `ready` names.
    ///
    /// # Errors
    ///
    /// The process does not start, or exits or is not ready within the bound `ready` names.
    pub fn start_with(
        binary: &Path,
        directory: &Path,
        config: &Path,
        deployment: &str,
        extra: &[String],
        ready: Ready,
    ) -> Result<Self> {
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let socket = directory.join("d.sock");
        let log = directory.join(format!("daemon-{}.log", ulid::Ulid::generate()));
        let _ = fs::remove_file(&socket);
        // `tracing_subscriber::fmt()` writes to stdout, so the log the cases read holds both.
        let stderr = fs::File::create(&log)?;
        let stdout = stderr.try_clone()?;
        let child = Command::new(binary)
            .args(["--driver", "kubernetes", "--kubernetes-config"])
            .arg(config)
            .arg("--socket")
            .arg(&socket)
            .arg("--state")
            .arg(directory.join("state.db"))
            .args(["--deployment", deployment, "--allow-uid"])
            .arg(nix_uid().to_string())
            .args(extra)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .env_remove("KUBECONFIG")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .with_context(|| format!("starting {}", binary.display()))?;
        let mut daemon = Self {
            child: Some(child),
            socket,
            log,
            sequence: 0,
        };
        let (bound, awaited) = match ready {
            Ready::Socket => (Duration::from_secs(30), "created no socket"),
            Ready::Ops(_) => (SERVING_BOUND, "answered no 200 on /readyz"),
        };
        let deadline = Instant::now() + bound;
        loop {
            let done = match ready {
                Ready::Socket => daemon.socket.exists(),
                Ready::Ops(ops) => readyz_answers_ok(ops),
            };
            if done {
                return Ok(daemon);
            }
            if let Some(child) = daemon.child.as_mut()
                && let Some(status) = child.try_wait()?
            {
                bail!(
                    "the daemon exited before it was ready ({status}): {}",
                    daemon.log()
                );
            }
            ensure!(
                Instant::now() < deadline,
                "the daemon {awaited} within {} s: {}",
                bound.as_secs(),
                daemon.log()
            );
            let pause = match ready {
                Ready::Socket => Duration::from_millis(50),
                Ready::Ops(_) => Duration::from_millis(500),
            };
            std::thread::sleep(pause);
        }
    }

    /// Everything the daemon wrote to stderr so far.
    pub fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The running daemon's process id.
    ///
    /// # Errors
    ///
    /// It was stopped or has exited.
    pub fn pid(&self) -> Result<u32> {
        self.child
            .as_ref()
            .map(Child::id)
            .ok_or_else(|| anyhow!("the daemon is not running"))
    }

    /// The status of an exit this call observed; `None` while the daemon runs, and for an exit an
    /// earlier call already returned. An exited daemon is reaped here, so [`Daemon::stop`] never
    /// signals its reused pid.
    ///
    /// # Errors
    ///
    /// The process could not be waited on.
    pub fn exit_status(&mut self) -> Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait()?;
        if status.is_some() {
            self.child = None;
        }
        Ok(status)
    }

    /// Stops the daemon with SIGINT, then SIGKILL after 15 s.
    pub fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            // SAFETY: `kill(2)` on the pid of our own child, which has not been reaped yet.
            unsafe {
                libc::kill(pid, libc::SIGINT);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// One request over the socket; the status and the JSON body.
    ///
    /// # Errors
    ///
    /// No connection, or an answer that is not a content-length JSON response.
    pub async fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value)> {
        self.sequence += 1;
        let request_id = format!("req_lane_{}", self.sequence);
        let mut stream = UnixStream::connect(&self.socket)
            .await
            .with_context(|| format!("connecting to {}", self.socket.display()))?;
        let payload = body.map(serde_json::to_vec).transpose()?;
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nx-request-id: {request_id}\r\n\
             Connection: close\r\n"
        );
        if let Some(payload) = &payload {
            let _ = write!(
                head,
                "content-type: application/json\r\ncontent-length: {}\r\n",
                payload.len()
            );
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await?;
        if let Some(payload) = &payload {
            stream.write_all(payload).await?;
        }
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_mins(2), stream.read_to_end(&mut answer))
            .await
            .map_err(|_| anyhow!("{method} {path} was not answered within 120 s"))??;
        let boundary = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| anyhow!("{method} {path}: no response head"))?;
        let head = String::from_utf8_lossy(&answer[..boundary]);
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|status| status.parse().ok())
            .ok_or_else(|| anyhow!("{method} {path}: no status line"))?;
        let body: Value = serde_json::from_slice(&answer[boundary + 4..])
            .with_context(|| format!("{method} {path}: the body is not JSON"))?;
        Ok((status, body))
    }

    /// `/v1/machine`'s result.
    ///
    /// # Errors
    ///
    /// The route did not answer 200.
    pub async fn machine(&mut self) -> Result<Value> {
        let (status, body) = self.call("GET", "/v1/machine", None).await?;
        ensure!(status == 200, "/v1/machine answered {status}: {body}");
        Ok(body["result"].clone())
    }

    /// Waits until the snapshot publishes `workspace.guarded-io`: the driver is open and its probe
    /// passed. The published facts.
    ///
    /// # Errors
    ///
    /// Not within [`SERVING_BOUND`].
    pub async fn serving(&mut self, stop: &AtomicBool) -> Result<BTreeSet<String>> {
        let deadline = Instant::now() + SERVING_BOUND;
        loop {
            ensure!(!stop.load(Ordering::SeqCst), "interrupted by a signal");
            let machine = self.machine().await?;
            if machine["facts"]["workspace.guarded-io"] == true {
                return Ok(published(&machine));
            }
            if Instant::now() >= deadline {
                bail!(
                    "the daemon published no probed snapshot within {} s: facts {:?}; codes {:?}; \
                     log tail {}",
                    SERVING_BOUND.as_secs(),
                    published(&machine),
                    named_codes(&self.log()),
                    tail(&self.log())
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Creates an empty workspace: its id, or the refusal's status and code.
    ///
    /// # Errors
    ///
    /// The daemon did not answer.
    pub async fn create_workspace(&mut self) -> Result<Result<String, (u16, String)>> {
        let body = json!({
            "op": ulid::Ulid::generate().to_string(),
            "input": {"source": "empty", "labels": {"lane": "namespace"}},
        });
        let (status, answer) = self.call("POST", "/v1/workspaces", Some(&body)).await?;
        if status == 201 {
            return Ok(Ok(answer["result"]["id"]
                .as_str()
                .ok_or_else(|| anyhow!("a created workspace names no id: {answer}"))?
                .to_owned()));
        }
        Ok(Err((status, code_of(&answer))))
    }

    /// Asks the daemon to destroy `workspace`: `Ok` once absent, else the refusal's status and
    /// code.
    ///
    /// # Errors
    ///
    /// The daemon did not answer.
    pub async fn destroy(&mut self, workspace: &str) -> Result<Result<(), (u16, String)>> {
        let body = json!({"op": ulid::Ulid::generate().to_string(), "input": {}});
        let (status, answer) = self
            .call(
                "DELETE",
                &format!("/v1/workspaces/{workspace}"),
                Some(&body),
            )
            .await?;
        if status == 200 && answer["result"]["absent"] == true {
            return Ok(Ok(()));
        }
        Ok(Err((status, code_of(&answer))))
    }

    /// Whether `workspace` reads absent: not found, or destroyed.
    ///
    /// # Errors
    ///
    /// The daemon did not answer.
    pub async fn absent(&mut self, workspace: &str) -> Result<bool> {
        let (status, answer) = self
            .call("GET", &format!("/v1/workspaces/{workspace}"), None)
            .await?;
        Ok(status == 404
            || answer["result"]["absent"] == true
            || matches!(
                answer["result"]["state"].as_str(),
                Some("destroyed" | "absent")
            ))
    }

    /// Destroys `workspace` and, when the answer is left to reconciliation, waits until it reads
    /// absent within [`ABSENCE_BOUND`].
    ///
    /// # Errors
    ///
    /// It is not absent in time.
    pub async fn destroy_until_absent(&mut self, workspace: &str) -> Result<()> {
        if let Ok(()) = self.destroy(workspace).await? {
            return Ok(());
        }
        let deadline = Instant::now() + ABSENCE_BOUND;
        while !self.absent(workspace).await? {
            ensure!(
                Instant::now() < deadline,
                "{workspace} was not absent within {} s; codes {:?}",
                ABSENCE_BOUND.as_secs(),
                named_codes(&self.log())
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Ok(())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The lane's own uid: the one peer each daemon it starts admits.
fn nix_uid() -> u32 {
    // SAFETY: `getuid(2)` cannot fail and touches no memory.
    unsafe { libc::getuid() }
}

fn code_of(answer: &Value) -> String {
    answer["error"]["code"]
        .as_str()
        .unwrap_or("<no code>")
        .to_owned()
}

fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(8)..].join(" | ")
}

/// The clean-room runner `build_daemon` builds into the daemon's own directory.
fn runner_beside(daemon: &Path) -> PathBuf {
    daemon.with_file_name(clean_room::PACKAGE)
}

/// A loopback address no listener holds now.
fn unused_loopback() -> Result<SocketAddr> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?.local_addr()?)
}

/// Whether `GET /readyz` on the plaintext operational listener at `ops` answers 200 within 2 s.
/// Any failure to connect, write or read is "not yet".
fn readyz_answers_ok(ops: SocketAddr) -> bool {
    use std::io::Read as _;
    let bound = Duration::from_secs(2);
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&ops, bound) else {
        return false;
    };
    if stream.set_read_timeout(Some(bound)).is_err()
        || stream.set_write_timeout(Some(bound)).is_err()
        || stream
            .write_all(
                format!("GET /readyz HTTP/1.1\r\nHost: {ops}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .is_err()
    {
        return false;
    }
    let mut head = [0_u8; 12];
    stream.read_exact(&mut head).is_ok() && status_is_ok(&head)
}

/// Whether an HTTP/1.x status line starts with status 200.
fn status_is_ok(head: &[u8]) -> bool {
    head.starts_with(b"HTTP/1.") && head.get(8..12) == Some(b" 200".as_slice())
}

/// Step 4's daemon arguments beside the lane's own: the production TLS listener with the
/// generated chain and key, the loopback Identity stub and its CA, and the operational listener.
fn tls_arguments(
    address: SocketAddr,
    ops: SocketAddr,
    chain: &Path,
    key: &Path,
    identity: &str,
    identity_ca: &Path,
) -> Vec<String> {
    [
        ("--tls-listen", address.to_string()),
        ("--tls-certificate-chain", chain.display().to_string()),
        ("--tls-private-key", key.display().to_string()),
        ("--hosted-identity-origin", identity.to_owned()),
        (
            "--hosted-identity-ca-bundle",
            identity_ca.display().to_string(),
        ),
        ("--ops-listen", ops.to_string()),
    ]
    .into_iter()
    .flat_map(|(flag, value)| [flag.to_owned(), value])
    .collect()
}

/// Step 4's runner call: the loopback TLS address verified as [`RUNNER_SERVER_NAME`], the
/// daemon's self-signed certificate as the one trust root, the kill of `pid` as the restart hook
/// and the operational listener.
fn runner_call<'a>(
    address: SocketAddr,
    ops: SocketAddr,
    roots: &'a Path,
    out: &'a Path,
    pid: u32,
) -> clean_room::Call<'a> {
    clean_room::Call {
        endpoint: format!("https://{address}/"),
        trust_roots: roots,
        server_name: RUNNER_SERVER_NAME,
        forbidden_name: RUNNER_FORBIDDEN_NAME,
        restart_hook: Some((KILL_PROGRAM, vec!["-KILL".to_owned(), pid.to_string()])),
        ops_origin: Some(format!("http://{ops}")),
        out,
        deadline: RUNNER_DEADLINE,
        io_timeout: RUNNER_IO,
        restart_timeout: RUNNER_RESTART,
    }
}

/// Why step 4's daemon exits are not exactly the one its restart hook caused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartFault {
    /// No daemon exit was observed, so the hook restarted nothing.
    HookCausedNoExit,
    /// An exit other than the hook's one `SIGKILL` of the first daemon.
    ExitNotCausedByHook,
}

impl std::fmt::Display for RestartFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::HookCausedNoExit => {
                "hook-caused-no-exit: the restart hook ran but no daemon exit was observed"
            }
            Self::ExitNotCausedByHook => {
                "daemon-exit-not-caused-by-hook: the daemon exited other than by the restart \
                 hook's one SIGKILL of the first daemon"
            }
        })
    }
}

/// The daemon restarts step 4 admits, from every daemon exit observed in order. The hook
/// (`runner_call`) sends `SIGKILL` once, to the first daemon's pid, so exactly one exit, by
/// `SIGKILL`, is the hook's; any further exit, or one by another cause, is not.
///
/// # Errors
///
/// The exits are not exactly that one.
pub fn hook_restarts(exits: &[ExitStatus]) -> Result<usize, RestartFault> {
    use std::os::unix::process::ExitStatusExt as _;
    match exits {
        [] => Err(RestartFault::HookCausedNoExit),
        [only] if only.signal() == Some(libc::SIGKILL) => Ok(1),
        _ => Err(RestartFault::ExitNotCausedByHook),
    }
}

/// What steps 3 to 7 need.
pub struct Steps<'a> {
    pub root: &'a Path,
    pub work: &'a Path,
    pub admin: &'a Api,
    /// The driver's kubeconfig for the lane namespace, with the extension.
    pub driver_config: &'a Path,
    /// The admin kubeconfig, from which the unlabelled namespace's driver kubeconfig is made.
    pub admin_config: &'a Value,
    pub settings: &'a Settings,
    pub namespace: &'a str,
    pub unlabelled: &'a str,
    /// Step 1's cluster-scoped inventory, after provisioning.
    pub inventory: &'a Inventory,
    pub stop: &'a AtomicBool,
}

/// Builds the shipped daemon in this checkout's profile and target directory, and step 5's
/// vectors test binary beside it; the daemon's path. Both are built before the cluster exists:
/// a build while the kind node runs starves it (run 2, 2026-10-29: containerd kill deadlines and
/// an apiserver liveness failure while step 5 compiled, and its first create past the driver's
/// handshake bound).
///
/// # Errors
///
/// A build or `cargo metadata` failed.
pub fn build_daemon(root: &Path, stop: &AtomicBool) -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    output(
        Command::new(&cargo).current_dir(root).args([
            "build",
            "--locked",
            "-p",
            DAEMON_PACKAGE,
            "--bin",
            "substrate-daemon",
        ]),
        stop,
    )?;
    output(
        Command::new(&cargo).current_dir(root).args([
            "test",
            "--locked",
            "--no-run",
            "-p",
            DAEMON_PACKAGE,
            "--test",
            "runtime_vectors",
        ]),
        stop,
    )?;
    // Its own invocation with `-p` alone, so no other package unifies the SDK's linked-daemon
    // feature back in (design 31 § 2).
    output(
        Command::new(&cargo).current_dir(root).args([
            "build",
            "--locked",
            "-p",
            clean_room::PACKAGE,
        ]),
        stop,
    )?;
    let metadata: Value = serde_json::from_str(&output(
        Command::new(&cargo).current_dir(root).args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--locked",
        ]),
        stop,
    )?)?;
    let target = metadata["target_directory"]
        .as_str()
        .ok_or_else(|| anyhow!("cargo metadata names no target directory"))?;
    let binary = Path::new(target).join("debug/substrate-daemon");
    ensure!(binary.is_file(), "{} was not built", binary.display());
    let runner = runner_beside(&binary);
    ensure!(runner.is_file(), "{} was not built", runner.display());
    Ok(binary)
}

impl Steps<'_> {
    fn lease_path(namespace: &str) -> String {
        format!("/apis/coordination.k8s.io/v1/namespaces/{namespace}/leases/{LEASE}")
    }

    /// Releases `namespace`'s Lease as admin once the daemon that held it has stopped.
    async fn release(&self, namespace: &str) -> Result<()> {
        self.admin
            .delete(&Self::lease_path(namespace), 0)
            .await
            .map_err(|error| anyhow!("releasing the Lease in {namespace}: {error}"))?;
        Ok(())
    }

    /// Steps 3 to 7, each case once.
    pub async fn run(&self, binary: &Path) -> Vec<(String, Case)> {
        let mut outcomes: Vec<(String, Case)> = Vec::new();
        // The run's directory is temporary; a failed case's daemon logs outlive it here.
        let kept = self
            .root
            .join(KEPT_LOGS)
            .join(ulid::Ulid::generate().to_string());
        let mut since = SystemTime::now();
        let mut record = |name: &str, case: Case| {
            let case = kept_case(self.work, &kept, name, since, case);
            since = SystemTime::now();
            match &case {
                Case::Passed(evidence) => println!("namespace lane: {name}: passed: {evidence}"),
                Case::Absent(reason) => println!("namespace lane: {name}: absent: {reason}"),
                Case::Failed(error) => println!("namespace lane: {name}: failed: {error}"),
            }
            outcomes.push((name.to_owned(), case));
        };
        for (name, reason) in ABSENT {
            record(name, Case::Absent(reason.to_owned()));
        }
        let lane_directory = self.work.join("lane");
        let full = match self.step_three(binary, &lane_directory).await {
            Ok((facts, evidence)) => {
                record(
                    "S3 daemon serves with --driver kubernetes",
                    Case::Passed(evidence),
                );
                facts
            }
            Err(error) => {
                record(
                    "S3 daemon serves with --driver kubernetes",
                    Case::Failed(format!("{error:#}")),
                );
                return outcomes;
            }
        };
        record(
            "S4 clean-room runner",
            to_case(self.step_four(binary, &lane_directory).await),
        );
        record(
            "S5 runtime vectors, namespace lane",
            to_case(self.step_five().await),
        );
        let mut lane = match self.lane_daemon(binary, &lane_directory).await {
            Ok(daemon) => daemon,
            Err(error) => {
                record(
                    "S6 same-named object of another deployment untouched",
                    Case::Failed(format!("the lane daemon did not serve again: {error:#}")),
                );
                return outcomes;
            }
        };
        record(
            "S6 same-named object of another deployment untouched",
            to_case(self.foreign_keeper(&mut lane, true).await),
        );
        record(
            "S6 foreign-uid object untouched",
            to_case(self.foreign_keeper(&mut lane, false).await),
        );
        record(
            "S6 quota workspace.capacity",
            to_case(self.quota(&mut lane).await),
        );
        lane.stop();
        let released = self.release(self.namespace).await;
        record(
            "S6 privileged dry run refuses startup",
            to_case(match released {
                Ok(()) => self.privileged(binary).await,
                Err(error) => Err(error),
            }),
        );
        let verbs_directory = self.work.join("verbs");
        for (resource, verb) in role_verbs() {
            let name = verb_case(&resource, &verb);
            if ABSENT.iter().any(|(absent, _)| *absent == name) {
                continue;
            }
            let case = self
                .verb(binary, &verbs_directory, &resource, &verb, &full)
                .await;
            let restored = self.set_role(None).await;
            record(
                &name,
                to_case(case.and_then(|evidence| restored.map(|()| evidence))),
            );
        }
        record(
            "S7 everything destroyed through the daemon",
            to_case(
                self.step_seven(binary, &lane_directory, &verbs_directory)
                    .await,
            ),
        );
        outcomes
    }

    /// Step 3: the lane daemon serves the probed snapshot; then it is stopped and its Lease
    /// released for the vectors' daemon.
    async fn step_three(
        &self,
        binary: &Path,
        directory: &Path,
    ) -> Result<(BTreeSet<String>, String)> {
        let mut daemon = Daemon::start(binary, directory, self.driver_config, LANE_DEPLOYMENT)?;
        let started = Instant::now();
        let facts = daemon.serving(self.stop).await?;
        let machine = daemon.machine().await?;
        ensure!(
            machine["driver"] == "kubernetes",
            "the snapshot names driver {}",
            machine["driver"]
        );
        ensure!(
            machine["facts"]["confinement.no-egress"].is_null(),
            "the snapshot publishes confinement.no-egress, which no probe here observes"
        );
        daemon.stop();
        self.release(self.namespace).await?;
        Ok((
            facts.clone(),
            format!(
                "Unix socket, driver kubernetes, probed snapshot after {} s: {facts:?}",
                started.elapsed().as_secs()
            ),
        ))
    }

    /// Step 4: design 31's runner against the lane's daemon over TLS. Core groups must pass and
    /// no case may fail; an absence is accepted only when the endpoint's facts or the runner's
    /// bounds decided it, or for the narrowed credential the stub does not issue.
    async fn step_four(&self, binary: &Path, directory: &Path) -> Result<String> {
        let runner = runner_beside(binary);
        ensure!(runner.is_file(), "{} was not built", runner.display());
        ensure!(
            Path::new(KILL_PROGRAM).is_file(),
            "the restart hook {KILL_PROGRAM} does not exist"
        );
        let material = Material::generate(
            vec![RUNNER_SERVER_NAME.to_owned()],
            RUNNER_FORBIDDEN_NAME.to_owned(),
            Ipv4Addr::LOCALHOST,
        )?;
        let identity = Listener::identity(Ipv4Addr::LOCALHOST, &material)?;
        let tally = self
            .certify(binary, &runner, directory, &material, &identity.origin())
            .await;
        identity.stop();
        let released = self.release(self.namespace).await;
        let evidence = tally?;
        released?;
        Ok(evidence)
    }

    async fn certify(
        &self,
        binary: &Path,
        runner: &Path,
        directory: &Path,
        material: &Material,
        identity: &str,
    ) -> Result<String> {
        let tls = self.work.join("clean-room");
        fs::create_dir_all(&tls)?;
        fs::set_permissions(&tls, fs::Permissions::from_mode(0o700))?;
        let chain = tls.join("daemon.pem");
        let key = tls.join("daemon-key.pem");
        let identity_ca = tls.join("identity.pem");
        fs::write(&chain, &material.daemon_pem)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key)?
            .write_all(material.daemon_key_pem.as_bytes())?;
        fs::write(&identity_ca, &material.identity_pem)?;
        let address = unused_loopback()?;
        let ops = unused_loopback()?;
        let arguments = tls_arguments(address, ops, &chain, &key, identity, &identity_ca);
        let start = || {
            Daemon::start_with(
                binary,
                directory,
                self.driver_config,
                LANE_DEPLOYMENT,
                &arguments,
                Ready::Ops(ops),
            )
        };
        let mut daemon = start()?;
        let out = self.work.join("conformance-report");
        let call = runner_call(address, ops, &chain, &out, daemon.pid()?);
        let token = clean_room::TokenSocket::serve(material.credential.clone())?;
        let mut running =
            clean_room::spawn(runner, &call, &token, &self.work.join("conformance.log"))?;
        let deadline = Instant::now() + call.bound();
        let mut exits = Vec::new();
        let exit = loop {
            if let Some(status) = running.exited()? {
                break status.code();
            }
            if self.stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
                running.kill();
                break None;
            }
            if let Some(status) = daemon.exit_status()? {
                exits.push(status);
                if hook_restarts(&exits) == Err(RestartFault::ExitNotCausedByHook) {
                    running.kill();
                    break None;
                }
                self.release(self.namespace).await?;
                daemon = start()?;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        // Whatever the verdict, step 4's workspaces go before step 5 starts.
        let swept = self.sweep(&mut daemon, start, address, material).await;
        daemon.stop();
        let answered = token.finish();
        let swept = match swept {
            Ok(swept) => self.left_by_step_four().await.map(|()| swept),
            Err(error) => Err(error),
        };
        // An exit the hook did not cause stopped the runner: that, not the stopped run, is the
        // failure. Otherwise the runner's verdict comes first, and only then the count.
        let restarts = hook_restarts(&exits);
        if let Err(fault @ RestartFault::ExitNotCausedByHook) = restarts {
            bail!("step 4: {fault}");
        }
        let tally = clean_room::judge(
            exit,
            &out,
            &self.work.join("conformance.log"),
            &material.credential,
            Some(&RUNNER_UNDECLARED),
        )?;
        let restarts = restarts.map_err(|fault| anyhow!("step 4: {fault}"))?;
        let swept = swept.context("step 4 left workspaces behind")?;
        Ok(format!(
            "{}; the daemon was started again {restarts} time(s); {answered} credential \
             request(s) answered; {} workspace(s) swept after the run",
            clean_room::evidence(&tally),
            swept.len()
        ))
    }

    /// Step 4's sweep through its own daemon, started again when the run left it stopped.
    async fn sweep(
        &self,
        daemon: &mut Daemon,
        start: impl Fn() -> Result<Daemon>,
        address: SocketAddr,
        material: &Material,
    ) -> Result<Vec<String>> {
        if daemon.exit_status()?.is_some() || daemon.pid().is_err() {
            self.release(self.namespace).await?;
            *daemon = start()?;
        }
        let client = material.client()?;
        let credential = material.credential.clone();
        tokio::task::spawn_blocking(move || {
            sweep_workspaces(
                &mut |method, path, body| {
                    tls_call(address, &client, &credential, (method, path, body))
                },
                ABSENCE_BOUND,
                Duration::from_secs(2),
            )
        })
        .await?
    }

    /// The lane deployment owns no Pod or claim once step 4's sweep is done, within
    /// [`ABSENCE_BOUND`].
    async fn left_by_step_four(&self) -> Result<()> {
        let deadline = Instant::now() + ABSENCE_BOUND;
        loop {
            let left = self.owned_by(LANE_DEPLOYMENT).await?;
            if left.is_empty() {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "{LANE_DEPLOYMENT} still owns {left:?} {} s after step 4's sweep",
                ABSENCE_BOUND.as_secs()
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Step 5: the vectors' namespace lane, selected by the driver's kubeconfig; its count must
    /// be the `NAMESPACE_CASES` the source pins.
    async fn step_five(&self) -> Result<String> {
        let source = fs::read_to_string(self.root.join(VECTORS_SOURCE))?;
        let declared = declared_namespace_cases(&source)
            .ok_or_else(|| anyhow!("{VECTORS_SOURCE} pins no NAMESPACE_CASES"))?;
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let ran = output(
            Command::new(cargo)
                .current_dir(self.root)
                .env(KUBERNETES_CONFIG_VARIABLE, self.driver_config)
                .env_remove("KUBECONFIG")
                .args([
                    "test",
                    "--locked",
                    "-p",
                    DAEMON_PACKAGE,
                    "--test",
                    "runtime_vectors",
                    "--",
                    "--exact",
                    VECTORS_TEST,
                    "--nocapture",
                ]),
            self.stop,
        );
        let released = self.release(self.namespace).await;
        let stdout = ran?;
        released?;
        let count = vectors_verdict(&stdout, declared).map_err(|error| anyhow!(error))?;
        Ok(format!(
            "{count} cases executed by {VECTORS_DEPLOYMENT}, NAMESPACE_CASES {declared}"
        ))
    }

    async fn lane_daemon(&self, binary: &Path, directory: &Path) -> Result<Daemon> {
        let mut daemon = Daemon::start(binary, directory, self.driver_config, LANE_DEPLOYMENT)?;
        daemon.serving(self.stop).await?;
        Ok(daemon)
    }

    fn pods(&self) -> String {
        format!("/api/v1/namespaces/{}/pods", self.namespace)
    }

    /// The keeper Pods labelled for `deployment`, by name.
    async fn keepers(&self, deployment: &str) -> Result<BTreeSet<String>> {
        let selector = format!(
            "{}={},{}=keeper",
            render::DEPLOYMENT_LABEL,
            render::deployment_digest(deployment),
            render::ROLE_LABEL
        );
        let listed = self
            .admin
            .list(
                &format!("{}?labelSelector={}", self.pods(), url_encoded(&selector)),
                "PodList",
                ListForm::Metadata,
                100,
                10,
            )
            .await
            .map_err(|error| anyhow!("listing keepers: {error}"))?;
        Ok(listed
            .items
            .iter()
            .filter_map(|item| item["metadata"]["name"].as_str().map(str::to_owned))
            .collect())
    }

    async fn wait_gone(&self, path: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_mins(2);
        loop {
            match self.admin.get(path).await {
                Ok(None) => return Ok(()),
                Ok(Some(_)) => {}
                Err(error) => bail!("reading {path}: {error}"),
            }
            ensure!(
                Instant::now() < deadline,
                "{path} was not gone within 120 s"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// A Pod that Pod Security `restricted` admits, named `name` with `labels`, that does nothing.
    fn sleeper(&self, name: &str, labels: &Value) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": name, "namespace": self.namespace, "labels": labels},
            "spec": {
                "automountServiceAccountToken": false,
                "restartPolicy": "Never",
                "terminationGracePeriodSeconds": 1,
                "activeDeadlineSeconds": 600,
                "securityContext": {
                    "runAsNonRoot": true,
                    "runAsUser": render::RUN_AS_USER,
                    "runAsGroup": render::RUN_AS_USER,
                    "seccompProfile": {"type": "RuntimeDefault"},
                },
                "containers": [{
                    "name": "foreign",
                    "image": self.settings.exec_image,
                    "imagePullPolicy": "IfNotPresent",
                    "command": ["sleep", "600"],
                    "securityContext": {
                        "allowPrivilegeEscalation": false,
                        "readOnlyRootFilesystem": true,
                        "capabilities": {"drop": ["ALL"]},
                    },
                    "resources": {
                        "requests": {"memory": render::MEMORY, "cpu": render::CPU},
                        "limits": {"memory": render::MEMORY, "cpu": render::CPU},
                    },
                }],
            },
        })
    }

    /// A workspace's keeper replaced by the admin with a same-named Pod, labelled for another
    /// deployment (`other_deployment`) or for this one under a new uid: the daemon's destroy must
    /// leave it untouched and refuse by name; once the admin removes it, the destroy completes.
    async fn foreign_keeper(&self, daemon: &mut Daemon, other_deployment: bool) -> Result<String> {
        let before = self.keepers(LANE_DEPLOYMENT).await?;
        let workspace = daemon
            .create_workspace()
            .await?
            .map_err(|(status, code)| anyhow!("creating the workspace answered {status} {code}"))?;
        let after = self.keepers(LANE_DEPLOYMENT).await?;
        let keeper = match after.difference(&before).collect::<Vec<_>>().as_slice() {
            [keeper] => (*keeper).clone(),
            other => bail!("the workspace's keeper is not one new Pod: {other:?}"),
        };
        let path = format!("{}/{keeper}", self.pods());
        let original = self
            .admin
            .get(&path)
            .await
            .map_err(|error| anyhow!("reading {keeper}: {error}"))?
            .ok_or_else(|| anyhow!("{keeper} is gone"))?;
        let mut labels = original["metadata"]["labels"].clone();
        if other_deployment {
            labels[render::DEPLOYMENT_LABEL] = json!(render::deployment_digest("dep_another"));
        }
        self.admin
            .delete(&path, 0)
            .await
            .map_err(|error| anyhow!("deleting {keeper}: {error}"))?;
        self.wait_gone(&path).await?;
        let foreign = self
            .admin
            .create(&self.pods(), &self.sleeper(&keeper, &labels))
            .await
            .map_err(|error| anyhow!("creating the foreign {keeper}: {error}"))?;
        let uid = foreign["metadata"]["uid"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let refused = daemon.destroy(&workspace).await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let kept = self
            .admin
            .get(&path)
            .await
            .map_err(|error| anyhow!("reading the foreign {keeper}: {error}"))?;
        let untouched = kept.as_ref().is_some_and(|pod| {
            pod["metadata"]["uid"] == uid.as_str() && pod["metadata"]["deletionTimestamp"].is_null()
        });
        let codes = named_codes(&daemon.log());
        let _ = self.admin.delete(&path, 0).await;
        self.wait_gone(&path).await?;
        let completed = async {
            let deadline = Instant::now() + ABSENCE_BOUND;
            while !daemon.absent(&workspace).await? {
                ensure!(
                    Instant::now() < deadline,
                    "the destroy did not complete within {} s once the foreign Pod was gone",
                    ABSENCE_BOUND.as_secs()
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let (status, code) = match refused {
            Ok(()) => bail!(
                "the destroy reported {workspace} absent while a foreign {keeper} held its name"
            ),
            Err(refusal) => refusal,
        };
        ensure!(
            untouched,
            "the foreign {keeper} was modified or deleted: {kept:?}"
        );
        ensure!(
            codes.contains("kubernetes.api-failed"),
            "the daemon's log names no refusal for the collision: {codes:?}"
        );
        completed?;
        Ok(format!(
            "{keeper} ({}) untouched by the destroy, answered {status} {code}, log names \
             kubernetes.api-failed; absent once the admin removed it",
            if other_deployment {
                "labelled for another deployment"
            } else {
                "same labels, foreign uid"
            }
        ))
    }

    /// Workspaces until the quota refuses: `workspace.capacity`, then every one destroyed.
    async fn quota(&self, daemon: &mut Daemon) -> Result<String> {
        let mut created = Vec::new();
        let mut refusal = None;
        for _ in 0..=render::QUOTA_CLAIMS {
            match daemon.create_workspace().await? {
                Ok(workspace) => created.push(workspace),
                Err(answer) => {
                    refusal = Some(answer);
                    break;
                }
            }
        }
        for workspace in &created {
            daemon.destroy_until_absent(workspace).await?;
        }
        match refusal {
            Some((status, code)) if code == "workspace.capacity" => Ok(format!(
                "workspace {} of a {}-claim quota answered {status} workspace.capacity; {} \
                 destroyed",
                created.len() + 1,
                render::QUOTA_CLAIMS,
                created.len()
            )),
            Some((status, code)) => bail!(
                "workspace {} answered {status} {code}, not workspace.capacity",
                created.len() + 1
            ),
            None => bail!(
                "{} workspaces were admitted under a {}-claim quota",
                created.len(),
                render::QUOTA_CLAIMS
            ),
        }
    }

    /// A driver for the unlabelled namespace, bound to the same Role there: the open sequence's
    /// privileged dry run is admitted, so startup is refused `kubernetes.pod-security-unenforced`
    /// and no driver fact is published.
    async fn privileged(&self, binary: &Path) -> Result<String> {
        let namespace = self.unlabelled;
        for object in render::provisioning(namespace, namespace)
            .into_iter()
            .filter(|object| {
                matches!(
                    object["kind"].as_str(),
                    Some("ServiceAccount" | "Role" | "RoleBinding" | "NetworkPolicy")
                )
            })
        {
            let kind = object["kind"].as_str().unwrap_or_default().to_owned();
            let collection = render::collection(&kind, namespace)
                .ok_or_else(|| anyhow!("no collection for {kind}"))?;
            self.admin
                .create(&collection, &object)
                .await
                .map_err(|error| anyhow!("creating the {kind} in {namespace}: {error}"))?;
        }
        let answer = self
            .admin
            .create(
                &format!(
                    "/api/v1/namespaces/{namespace}/serviceaccounts/{}/token",
                    render::DRIVER
                ),
                &json!({
                    "apiVersion": "authentication.k8s.io/v1",
                    "kind": "TokenRequest",
                    "spec": {"expirationSeconds": 3600},
                }),
            )
            .await
            .map_err(|error| anyhow!("requesting the token in {namespace}: {error}"))?;
        let token = answer["status"]["token"]
            .as_str()
            .ok_or_else(|| anyhow!("the token request answered no token"))?;
        let config = with_extension(
            crate::namespace_lane::driver_config(self.admin_config, token, namespace),
            self.settings,
        );
        let path = self.work.join("unlabelled-driver.json");
        crate::namespace_lane::write_private(&path, &config)?;
        let mut daemon = Daemon::start(
            binary,
            &self.work.join("unlabelled"),
            &path,
            UNLABELLED_DEPLOYMENT,
        )?;
        let deadline = Instant::now() + REFUSAL_BOUND;
        let code = "kubernetes.pod-security-unenforced";
        while !named_codes(&daemon.log()).contains(code) {
            ensure!(
                Instant::now() < deadline,
                "startup in {namespace} was not refused {code} within {} s: codes {:?}",
                REFUSAL_BOUND.as_secs(),
                named_codes(&daemon.log())
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let facts = published(&daemon.machine().await?);
        daemon.stop();
        self.release(namespace).await?;
        for driver_fact in [
            "workspace.guarded-io",
            "workspace.max-current",
            "exec.max-current",
        ] {
            ensure!(
                !facts.contains(driver_fact),
                "{driver_fact} is published by a driver whose startup was refused"
            );
        }
        Ok(format!("startup refused {code}; published only {facts:?}"))
    }

    /// The lane's Role with `without` (resource, verb) removed, or decision 3's whole table.
    async fn set_role(&self, without: Option<(&str, &str)>) -> Result<()> {
        let path = format!(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{}/roles/{}",
            self.namespace,
            render::DRIVER
        );
        let mut role = self
            .admin
            .get(&path)
            .await
            .map_err(|error| anyhow!("reading the Role: {error}"))?
            .ok_or_else(|| anyhow!("the Role is gone"))?;
        let rules: Vec<Value> = render::role_grants()
            .into_iter()
            .filter_map(|grant| {
                let verbs: Vec<String> = grant
                    .verbs
                    .into_iter()
                    .filter(|verb| without != Some((grant.resource.as_str(), verb.as_str())))
                    .collect();
                (!verbs.is_empty()).then(|| {
                    json!({"apiGroups": [grant.group], "resources": [grant.resource], "verbs": verbs})
                })
            })
            .collect();
        role["rules"] = json!(rules);
        self.admin
            .update(&path, &role)
            .await
            .map_err(|error| anyhow!("updating the Role: {error}"))?;
        Ok(())
    }

    /// One Role verb removed: once the first probe has finished, a fact the whole Role publishes
    /// is absent and a `WARN` line carries the API's answer refusing this verb ([`verb_refusal`]),
    /// or the route that needs the verb is refused by name.
    async fn verb(
        &self,
        binary: &Path,
        directory: &Path,
        resource: &str,
        verb: &str,
        full: &BTreeSet<String>,
    ) -> Result<String> {
        self.set_role(Some((resource, verb))).await?;
        let mut daemon = Daemon::start(binary, directory, self.driver_config, VERBS_DEPLOYMENT)?;
        let started = Instant::now();
        let mut route: Option<String> = None;
        let mut leftover: Option<String> = None;
        let outcome = loop {
            if self.stop.load(Ordering::SeqCst) {
                break Err(anyhow!("interrupted by a signal"));
            }
            // The log first: a probe it shows finished is one the snapshot read after it holds.
            let log = daemon.log();
            let facts = published(&daemon.machine().await?);
            let codes = named_codes(&log);
            let missing: BTreeSet<String> = full.difference(&facts).cloned().collect();
            if started.elapsed() > Duration::from_secs(2)
                && let Some(refused) = verb_refusal(&log, 0, resource, verb, &missing)
            {
                break Ok(refused);
            }
            if route.is_none() && missing.is_empty() && started.elapsed() >= VERB_ROUTE_AFTER {
                let (answer, created) = self.route(&mut daemon).await?;
                leftover = created;
                if let Some(refused) = answer {
                    break Ok(format!(
                        "facts whole; {refused}; the log names {:?}",
                        named_codes(&daemon.log())
                    ));
                }
                route = Some("create and destroy both answered".to_owned());
            }
            if started.elapsed() >= VERB_BOUND {
                break Err(anyhow!(
                    "nothing refused within {} s: no WARN line after the first probe carries {:?}; \
                     facts {facts:?}, codes {codes:?}, route {route:?}",
                    VERB_BOUND.as_secs(),
                    api_refusal(resource, verb)
                ));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        if let Some(workspace) = leftover {
            self.set_role(None).await?;
            daemon.destroy_until_absent(&workspace).await?;
        }
        daemon.stop();
        self.release(self.namespace).await?;
        outcome.map(|evidence| format!("after {} s: {evidence}", started.elapsed().as_secs()))
    }

    /// The workspace routes: a refusal by name, if either answered one, and the workspace left
    /// to destroy once the Role is whole again.
    async fn route(&self, daemon: &mut Daemon) -> Result<(Option<String>, Option<String>)> {
        let workspace = match daemon.create_workspace().await? {
            Ok(workspace) => workspace,
            Err((status, code)) => {
                return Ok((
                    Some(format!("workspace.create answered {status} {code}")),
                    None,
                ));
            }
        };
        let since = daemon.log().len();
        match daemon.destroy(&workspace).await? {
            Ok(()) => Ok((None, None)),
            // Pending is the daemon's ordinary answer while the keeper terminates, not a refusal:
            // reconciliation goes on, and either finishes or names why it cannot.
            Err((500, code)) if code == OUTCOME_UNKNOWN => {
                let deadline = Instant::now() + PENDING_BOUND;
                loop {
                    if daemon.absent(&workspace).await? {
                        return Ok((None, None));
                    }
                    let named = pending_destroy_codes(&daemon.log(), since, &workspace);
                    if !named.is_empty() {
                        return Ok((
                            Some(format!(
                                "workspace.destroy stayed pending and reconciliation names \
                                 {named:?}"
                            )),
                            Some(workspace),
                        ));
                    }
                    ensure!(
                        Instant::now() < deadline,
                        "workspace.destroy stayed pending {} s and named no code",
                        PENDING_BOUND.as_secs()
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            Err((status, code)) => Ok((
                Some(format!("workspace.destroy answered {status} {code}")),
                Some(workspace),
            )),
        }
    }

    /// Step 7: the verbs' and the lane's daemons run once more with the whole Role, so their
    /// probes sweep what a verb case left and their reconciliation finishes; then, as admin, no
    /// labelled object but each namespace's Lease, and after releasing them none at all; and the
    /// cluster-scoped inventory equals step 1's.
    async fn step_seven(&self, binary: &Path, lane: &Path, verbs: &Path) -> Result<String> {
        for (directory, deployment) in [(verbs, VERBS_DEPLOYMENT), (lane, LANE_DEPLOYMENT)] {
            if !directory.join("state.db").exists() {
                continue;
            }
            let mut daemon = Daemon::start(binary, directory, self.driver_config, deployment)?;
            daemon.serving(self.stop).await?;
            let deadline = Instant::now() + ABSENCE_BOUND;
            loop {
                let found = self.labelled(self.namespace).await?;
                let theirs = self.owned_by(deployment).await?;
                if theirs.is_empty() {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "{deployment} still owns {theirs:?} {} s after it served again ({found:?})",
                    ABSENCE_BOUND.as_secs()
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            daemon.stop();
            if deployment == VERBS_DEPLOYMENT {
                // The lane's daemon claims the namespace next: another deployment's Lease would
                // refuse it for ever. The lane's own claim is left for the assertion below.
                self.release(self.namespace).await?;
            }
        }
        let mut kept = Vec::new();
        for namespace in [self.namespace, self.unlabelled] {
            let found = self.labelled(namespace).await?;
            let unexpected = unexpected_leftovers(&found);
            ensure!(
                unexpected.is_empty(),
                "{namespace} still holds {unexpected:?}"
            );
            kept.extend(
                found
                    .into_iter()
                    .map(|object| format!("{namespace}/{object}")),
            );
            self.release(namespace).await?;
            let after = self.labelled(namespace).await?;
            ensure!(
                after.is_empty(),
                "{namespace} still holds {after:?} after the release"
            );
        }
        let deadline = Instant::now() + INVENTORY_BOUND;
        loop {
            let now = inventory(self.admin).await?;
            let differences = inventory_difference(self.inventory, &now);
            if differences.is_empty() {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "the cluster-scoped inventory differs from step 1's: {differences:?}"
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Ok(format!(
            "no labelled object remained but the released claim {kept:?}; cluster-scoped \
             inventory equal to step 1's"
        ))
    }

    /// Every object in `namespace` carrying the managed-by label, as `Kind/name`.
    async fn labelled(&self, namespace: &str) -> Result<Vec<String>> {
        let selector = format!("{}={}", render::MANAGED_BY, render::MANAGER);
        let mut found = Vec::new();
        for (path, list, kind) in labelled_collections(namespace) {
            let listed = self
                .admin
                .list(
                    &format!("{path}?labelSelector={}", url_encoded(&selector)),
                    list,
                    ListForm::Metadata,
                    100,
                    10,
                )
                .await
                .map_err(|error| anyhow!("listing {list}: {error}"))?;
            found.extend(
                listed
                    .items
                    .iter()
                    .filter_map(|item| item["metadata"]["name"].as_str())
                    .map(|name| format!("{kind}/{name}")),
            );
        }
        Ok(found)
    }

    /// The Pods and claims in the lane namespace labelled for `deployment`.
    async fn owned_by(&self, deployment: &str) -> Result<Vec<String>> {
        let selector = format!(
            "{}={}",
            render::DEPLOYMENT_LABEL,
            render::deployment_digest(deployment)
        );
        let mut found = Vec::new();
        for (path, list, kind) in labelled_collections(self.namespace).into_iter().take(2) {
            let listed = self
                .admin
                .list(
                    &format!("{path}?labelSelector={}", url_encoded(&selector)),
                    list,
                    ListForm::Metadata,
                    100,
                    10,
                )
                .await
                .map_err(|error| anyhow!("listing {list}: {error}"))?;
            found.extend(
                listed
                    .items
                    .iter()
                    .filter_map(|item| item["metadata"]["name"].as_str())
                    .map(|name| format!("{kind}/{name}")),
            );
        }
        Ok(found)
    }
}

/// The cluster-scoped inventory: every name of each collection
/// [`crate::namespace_lane_observe::INVENTORY`] lists.
///
/// # Errors
///
/// A list failed.
pub async fn inventory(admin: &Api) -> Result<Inventory> {
    let mut names = Inventory::new();
    for (path, kind) in crate::namespace_lane_observe::INVENTORY {
        let listed = admin
            .list(path, kind, ListForm::Metadata, 500, 20)
            .await
            .map_err(|error| anyhow!("{kind}: {error}"))?;
        names.insert(
            kind.to_owned(),
            listed
                .items
                .iter()
                .filter_map(|item| item["metadata"]["name"].as_str().map(str::to_owned))
                .collect(),
        );
    }
    Ok(names)
}

fn to_case(result: Result<String>) -> Case {
    match result {
        Ok(evidence) => Case::Passed(evidence),
        Err(error) => Case::Failed(format!("{error:#}")),
    }
}

/// `case`; when it failed, with the daemon logs written while it ran kept under `kept`
/// ([`keep_daemon_logs`]) and their paths named in its failure, so a run that is no record can
/// still be read once the run's temporary directory is gone.
fn kept_case(work: &Path, kept: &Path, name: &str, since: SystemTime, case: Case) -> Case {
    let Case::Failed(error) = case else {
        return case;
    };
    Case::Failed(match keep_daemon_logs(work, kept, name, since) {
        Ok(logs) if logs.is_empty() => format!("{error}; no daemon log was written during it"),
        Ok(logs) => {
            let paths: Vec<String> = logs.iter().map(|log| log.display().to_string()).collect();
            let noun = if paths.len() == 1 { "log" } else { "logs" };
            format!("{error}; daemon {noun} kept at {}", paths.join(", "))
        }
        Err(kept_error) => format!("{error}; its daemon logs were not kept: {kept_error}"),
    })
}

/// Copies every `daemon-*.log` in a directory of `work` last written at or after `since`, less
/// [`MTIME_SLACK`] for the kernel's coarse file clock, into `kept/<case>/` (owner-private), `case` cut to
/// lower-case letters, digits and dashes; the copies' paths, in name order.
///
/// # Errors
///
/// A directory could not be read or created, or a log could not be copied.
pub fn keep_daemon_logs(
    work: &Path,
    kept: &Path,
    case: &str,
    since: SystemTime,
) -> std::io::Result<Vec<PathBuf>> {
    let since = since
        .checked_sub(MTIME_SLACK)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut logs = Vec::new();
    for directory in fs::read_dir(work)? {
        let directory = directory?.path();
        if !directory.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("daemon-")
                && Path::new(&name)
                    .extension()
                    .is_some_and(|extension| extension == "log")
                && entry.metadata()?.modified()? >= since
            {
                logs.push((name, entry.path()));
            }
        }
    }
    logs.sort();
    if logs.is_empty() {
        return Ok(Vec::new());
    }
    let slug = case
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let target = kept.join(slug);
    fs::create_dir_all(&target)?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
    logs.into_iter()
        .map(|(name, log)| {
            let copy = target.join(name);
            fs::copy(&log, &copy)?;
            Ok(copy)
        })
        .collect()
}

/// One request to a daemon: the status and the JSON body.
pub type Call<'a> = dyn FnMut(&str, &str, Option<&Value>) -> Result<(u16, Value)> + 'a;

/// Step 4's sweep: every workspace the runner's subject holds is destroyed through the daemon
/// and reads absent within `bound`, whatever the run's verdict, so steps 5 and 6 never meet the
/// namespace quota because of step 4. The workspaces come from a reconciliation snapshot, the
/// public listing of the subject's current resources. A destroy the driver leaves to
/// reconciliation answers `operation.outcome-unknown` and is waited out (design 34 § 5); a
/// `destroying` workspace is only waited out, never destroyed again under a fresh id (design 01
/// § 6.2). The swept ids.
///
/// # Errors
///
/// A request failed, a destroy was refused, or a workspace still read present after `bound`.
pub fn sweep_workspaces(
    call: &mut Call<'_>,
    bound: Duration,
    pause: Duration,
) -> Result<Vec<String>> {
    let (status, created) = call("POST", "/v1/reconciliation-snapshots", Some(&json!({})))?;
    ensure!(
        status == 201,
        "the reconciliation snapshot answered {status}: {}",
        code_of(&created)
    );
    let snapshot = created["result"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("the reconciliation snapshot names no id: {created}"))?
        .to_owned();
    let mut held = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut path = format!("/v1/reconciliation-snapshots/{snapshot}?limit=1000");
        if let Some(cursor) = &cursor {
            let _ = write!(path, "&cursor={}", url_encoded(cursor));
        }
        let (status, page) = call("GET", &path, None)?;
        ensure!(
            status == 200,
            "the snapshot page answered {status}: {}",
            code_of(&page)
        );
        for item in page["result"]["items"].as_array().into_iter().flatten() {
            if item["kind"] == "workspace"
                && let Some(id) = item["id"].as_str()
            {
                let state = item["value"]["state"].as_str().unwrap_or_default();
                held.push((id.to_owned(), state.to_owned()));
            }
        }
        match page["result"]["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    for (id, state) in &held {
        if state == "destroying" || state == "destroyed" {
            continue;
        }
        let body = json!({"op": ulid::Ulid::generate().to_string(), "input": {}});
        let (status, answer) = call("DELETE", &format!("/v1/workspaces/{id}"), Some(&body))?;
        let code = code_of(&answer);
        let settled = matches!(
            (status, code.as_str()),
            (200 | 404, _) | (500, OUTCOME_UNKNOWN) | (409, "workspace.not-ready")
        );
        ensure!(settled, "destroying {id} answered {status} {code}");
    }
    let deadline = Instant::now() + bound;
    for (id, _) in &held {
        loop {
            let (status, answer) = call("GET", &format!("/v1/workspaces/{id}"), None)?;
            if status == 404 || answer["result"]["state"] == "destroyed" {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "{id} still read {status} {} {} s after step 4's sweep began",
                answer["result"]["state"],
                bound.as_secs()
            );
            std::thread::sleep(pause);
        }
    }
    Ok(held.into_iter().map(|(id, _)| id).collect())
}

/// One request to the TLS daemon step 4 started, as the runner's subject: `credential` is the
/// bearer the lane's Identity stub resolves to it.
///
/// # Errors
///
/// No connection, a TLS failure, or an answer without a status line or a JSON body.
pub fn tls_call(
    address: SocketAddr,
    client: &std::sync::Arc<rustls::ClientConfig>,
    credential: &str,
    request: (&str, &str, Option<&Value>),
) -> Result<(u16, Value)> {
    let (method, path, body) = request;
    let stream = std::net::TcpStream::connect(address)?;
    stream.set_read_timeout(Some(RUNNER_IO))?;
    let server = rustls::pki_types::ServerName::try_from(RUNNER_SERVER_NAME.to_owned())?;
    let connection = rustls::ClientConnection::new(std::sync::Arc::clone(client), server)?;
    let mut tls = rustls::StreamOwned::new(connection, stream);
    let payload = body.map(Value::to_string).unwrap_or_default();
    let content = if payload.is_empty() {
        String::new()
    } else {
        format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            payload.len()
        )
    };
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {RUNNER_SERVER_NAME}\r\nauthorization: Bearer \
         {credential}\r\n{content}connection: close\r\n\r\n{payload}"
    );
    std::io::Write::write_all(&mut tls, request.as_bytes())?;
    let mut answer = Vec::new();
    if let Err(error) = std::io::Read::read_to_end(&mut tls, &mut answer)
        && (error.kind() != std::io::ErrorKind::UnexpectedEof || answer.is_empty())
    {
        return Err(anyhow!("{method} {path}: {error}"));
    }
    let boundary = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("{method} {path}: no response head"))?;
    let head = String::from_utf8_lossy(&answer[..boundary]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| anyhow!("{method} {path}: no status line"))?;
    let body: Value = serde_json::from_slice(&answer[boundary + 4..])
        .with_context(|| format!("{method} {path}: the body is not JSON"))?;
    Ok((status, body))
}

fn url_encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> Vec<(String, Case)> {
        cases()
            .into_iter()
            .map(|name| {
                let case = ABSENT
                    .iter()
                    .find(|(absent, _)| *absent == name)
                    .map_or_else(
                        || Case::Passed("evidence".to_owned()),
                        |(_, reason)| Case::Absent((*reason).to_owned()),
                    );
                (name, case)
            })
            .collect()
    }

    /// A stand-in daemon that ignores the lane's arguments and creates no socket, as the shipped
    /// daemon does with `--tls-listen`: `/bin/true`, or `/bin/false` when it must fail. A system
    /// program, not a script the test writes: an executable written while another test forks
    /// can be held open for writing by that child, and its `exec` then fails `ETXTBSY`.
    fn stand_in(fails: bool) -> PathBuf {
        PathBuf::from(if fails { "/bin/false" } else { "/bin/true" })
    }

    /// An operational listener that answers every request with `status`.
    fn ops_answering(status: &'static str) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = [0_u8; 256];
                let _ = std::io::Read::read(&mut stream, &mut request);
                let _ = stream.write_all(
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\n\r\n").as_bytes(),
                );
            }
        });
        address
    }

    #[test]
    fn a_tls_daemon_is_ready_by_its_readyz_though_it_creates_no_socket() {
        let directory = tempfile::tempdir().expect("tempdir");
        let binary = stand_in(false);
        let ops = ops_answering("200 OK");
        let started = Instant::now();
        let daemon = Daemon::start_with(
            &binary,
            &directory.path().join("d"),
            &directory.path().join("config.json"),
            LANE_DEPLOYMENT,
            &[],
            Ready::Ops(ops),
        )
        .expect("ready by /readyz");
        assert!(
            !daemon.socket.exists(),
            "the stand-in creates no socket, as a TLS daemon does not"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_readyz_503_is_not_ready_and_a_200_is() {
        assert!(!readyz_answers_ok(ops_answering("503 Service Unavailable")));
        assert!(readyz_answers_ok(ops_answering("200 OK")));
        assert!(!readyz_answers_ok(unused_loopback().expect("a free port")));
        assert!(status_is_ok(b"HTTP/1.1 200"));
        assert!(!status_is_ok(b"XTTP/1.1 200"));
    }

    #[test]
    fn a_daemon_that_exits_before_its_readyz_answers_is_named() {
        let directory = tempfile::tempdir().expect("tempdir");
        let binary = stand_in(true);
        let ops = unused_loopback().expect("a free port");
        let error = Daemon::start_with(
            &binary,
            &directory.path().join("d"),
            &directory.path().join("config.json"),
            LANE_DEPLOYMENT,
            &[],
            Ready::Ops(ops),
        )
        .err()
        .expect("an exited daemon is refused");
        let error = format!("{error:#}");
        assert!(
            error.contains("exited before it was ready"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn every_role_verb_of_decision_three_is_a_case_and_step_four_must_pass() {
        let names = cases();
        assert_eq!(role_verbs().len(), 17, "decision 3 grants 17 verbs");
        for (resource, verb) in role_verbs() {
            assert!(
                names.contains(&verb_case(&resource, &verb)),
                "{resource} {verb}"
            );
        }
        assert!(names.contains(&"S4 clean-room runner".to_owned()));
        assert!(
            !ABSENT.iter().any(|(absent, _)| absent.starts_with("S4")),
            "step 4 is run, never declared absent, once the runner exists"
        );
        for unused in ["persistentvolumeclaims list", "networkpolicies get"] {
            let name = format!("S6 Role without {unused}");
            assert!(
                ABSENT.iter().any(|(absent, reason)| *absent == name
                    && reason.contains("decision 3 grants a verb the driver does not use")),
                "{name} has no dependent and is named absent as a finding"
            );
        }
        for (absent, _) in ABSENT {
            assert!(
                names.contains(&absent.to_owned()),
                "{absent} is a declared case"
            );
        }
        assert_eq!(verdict(&passing()), None);
    }

    /// Finding `namespace-lane-restarts-unpinned`: the hook sends `SIGKILL` to the first daemon
    /// once, so step 4 admits exactly that one exit; any other exit, or none, fails by name.
    #[test]
    fn step_four_admits_exactly_the_daemon_exit_its_restart_hook_caused() {
        use std::os::unix::process::ExitStatusExt as _;
        let killed = ExitStatus::from_raw(libc::SIGKILL);
        let terminated = ExitStatus::from_raw(libc::SIGTERM);
        let exited = ExitStatus::from_raw(1 << 8);
        assert_eq!(hook_restarts(&[killed]), Ok(1));
        assert_eq!(
            hook_restarts(&[]),
            Err(RestartFault::HookCausedNoExit),
            "a run whose hook killed nothing was not restarted by it"
        );
        assert_eq!(
            hook_restarts(&[killed, killed]),
            Err(RestartFault::ExitNotCausedByHook),
            "the hook names the first daemon's pid; a second exit is not its doing"
        );
        for status in [terminated, exited] {
            assert_eq!(
                hook_restarts(&[status]),
                Err(RestartFault::ExitNotCausedByHook),
                "{status}: the hook sends SIGKILL only"
            );
        }
        assert!(
            RestartFault::ExitNotCausedByHook
                .to_string()
                .contains("daemon-exit-not-caused-by-hook")
        );
        assert!(
            RestartFault::HookCausedNoExit
                .to_string()
                .contains("hook-caused-no-exit")
        );
    }

    #[test]
    fn an_absent_case_reported_passed_a_failure_or_a_missing_case_is_no_record() {
        let mut passed_absent = passing();
        let quota = passed_absent
            .iter_mut()
            .find(|(name, _)| name == "S6 quota exec.capacity")
            .expect("an absent case");
        quota.1 = Case::Passed("ran".to_owned());
        assert!(verdict(&passed_absent).is_some_and(|fault| fault.contains("exec.capacity")));

        let mut step_four_absent = passing();
        let step_four = step_four_absent
            .iter_mut()
            .find(|(name, _)| name == "S4 clean-room runner")
            .expect("step 4");
        step_four.1 = Case::Absent("postponed".to_owned());
        assert!(
            verdict(&step_four_absent)
                .is_some_and(|fault| fault.contains("S4 clean-room runner is not declared absent"))
        );

        let mut failed = passing();
        failed[0].1 = Case::Failed("no snapshot".to_owned());
        assert!(verdict(&failed).is_some_and(|fault| fault.contains("no snapshot")));

        let mut missing = passing();
        missing.retain(|(name, _)| !name.starts_with("S7"));
        assert!(verdict(&missing).is_some_and(|fault| fault.contains("S7")));

        let mut absent_unnamed = passing();
        absent_unnamed[2].1 = Case::Absent("too slow".to_owned());
        assert!(verdict(&absent_unnamed).is_some());

        let mut twice = passing();
        twice.push(twice[0].clone());
        assert!(verdict(&twice).is_some_and(|fault| fault.contains("more than once")));
    }

    /// Step 4's runner: dialled on loopback, verified under the certificate's own name and
    /// refused under one it does not carry, restarted by `kill -KILL` of the daemon's pid, argv
    /// only, and bounded within the runner's own ceilings (3600 s, 60 s, 600 s).
    #[test]
    fn step_four_runs_the_runner_built_beside_the_daemon_with_a_kill_hook_within_its_ceilings() {
        assert_eq!(
            runner_beside(Path::new("/w/target/debug/substrate-daemon")),
            Path::new("/w/target/debug/b10x-substrate-conformance")
        );
        let address: SocketAddr = "127.0.0.1:4443".parse().expect("address");
        let ops: SocketAddr = "127.0.0.1:9090".parse().expect("address");
        let call = runner_call(
            address,
            ops,
            Path::new("/w/clean-room/daemon.pem"),
            Path::new("/w/conformance-report"),
            4242,
        );
        let argv = clean_room::argv(&call, 9);
        let after = |flag: &str| {
            argv.iter()
                .position(|argument| argument == flag)
                .map(|index| argv[index + 1].as_str())
        };
        assert_eq!(after("--endpoint"), Some("https://127.0.0.1:4443/"));
        assert_eq!(after("--server-name"), Some(RUNNER_SERVER_NAME));
        assert_eq!(after("--forbidden-name"), Some(RUNNER_FORBIDDEN_NAME));
        assert_eq!(after("--trust-roots"), Some("/w/clean-room/daemon.pem"));
        assert_eq!(after("--restart-hook"), Some("/usr/bin/kill"));
        assert_eq!(after("--ops-origin"), Some("http://127.0.0.1:9090"));
        assert_eq!(after("--token-fd"), Some("9"));
        let hook: Vec<&str> = argv
            .windows(2)
            .filter(|pair| pair[0] == "--restart-hook-arg")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(hook, ["-KILL", "4242"]);
        assert!(RUNNER_DEADLINE.as_secs() <= 3_600);
        assert!(RUNNER_IO.as_secs() <= 60);
        assert!(RUNNER_RESTART.as_secs() <= 600 && RUNNER_RESTART > SERVING_BOUND);
        assert_ne!(RUNNER_SERVER_NAME, RUNNER_FORBIDDEN_NAME);

        let daemon = tls_arguments(
            address,
            ops,
            Path::new("/w/chain.pem"),
            Path::new("/w/key.pem"),
            "https://127.0.0.1:7000",
            Path::new("/w/identity.pem"),
        );
        assert_eq!(
            daemon,
            [
                "--tls-listen",
                "127.0.0.1:4443",
                "--tls-certificate-chain",
                "/w/chain.pem",
                "--tls-private-key",
                "/w/key.pem",
                "--hosted-identity-origin",
                "https://127.0.0.1:7000",
                "--hosted-identity-ca-bundle",
                "/w/identity.pem",
                "--ops-listen",
                "127.0.0.1:9090",
            ]
        );
    }

    #[test]
    fn the_vectors_count_must_be_the_pinned_one_from_one_selected_passing_case() {
        let ok =
            format!("running 1 test\n{VECTORS_LINE}61 cases\ntest result: ok. 1 passed; 0 failed");
        assert_eq!(vectors_verdict(&ok, 61), Ok(61));
        assert!(vectors_verdict(&ok, 60).is_err());
        let none = "running 0 tests\ntest result: ok. 0 passed; 0 failed";
        assert!(vectors_verdict(none, 61).is_err());
        let silent = "running 1 test\ntest result: ok. 1 passed; 0 failed";
        assert!(vectors_verdict(silent, 61).is_err());
        assert_eq!(
            declared_namespace_cases("const X: usize = 1;\nconst NAMESPACE_CASES: usize = 61;\n"),
            Some(61)
        );
        assert_eq!(
            declared_namespace_cases("const NAMESPACE_CASES: usize = x;"),
            None
        );
    }

    #[test]
    fn the_repositorys_vectors_pin_namespace_cases() {
        let source = include_str!("../../crates/substrate-daemon/tests/runtime_vectors.rs");
        assert!(declared_namespace_cases(source).is_some_and(|count| count > 0));
        assert!(source.contains(KUBERNETES_CONFIG_VARIABLE));
        assert!(source.contains(&format!("fn {VECTORS_TEST}()")));
        assert!(source.contains(&format!("\"{VECTORS_DEPLOYMENT}\"")));
    }

    #[test]
    fn warn_lines_name_their_codes_and_other_lines_do_not() {
        let log = "2026-10-29T00:00:00Z  WARN substrate_kubernetes: the namespace driver is not \
                   open code=\"kubernetes.api-failed\" message=x\n\
                   2026-10-29T00:00:00Z  INFO substrate_kubernetes: open code=\"kubernetes.other\"\n\
                   2026-10-29T00:00:01Z  WARN substrate_daemon: pending code=kubernetes.lease-held";
        assert_eq!(
            named_codes(log),
            BTreeSet::from([
                "kubernetes.api-failed".to_owned(),
                "kubernetes.lease-held".to_owned()
            ])
        );
        let coloured = "\u{1b}[33m WARN\u{1b}[0m \u{1b}[2msubstrate_daemon\u{1b}[0m: pending \
                        \u{1b}[3mcode\u{1b}[0m\u{1b}[2m=\u{1b}[0m\"kubernetes.api-failed\"";
        assert_eq!(
            named_codes(coloured),
            BTreeSet::from(["kubernetes.api-failed".to_owned()])
        );
    }

    /// The PVC-delete Role case: the probe's teardown named `kubernetes.api-failed` before the
    /// destroy, and reconciliation names the same code again for the workspace afterwards. That
    /// second naming is the refusal; a code the log held before the destroy, or a line about
    /// another object, is not.
    #[test]
    fn a_pending_destroy_is_named_by_a_warn_for_its_workspace_written_after_it() {
        let before = "2026-10-29T12:30:00Z  WARN substrate_kubernetes::namespace::probe: a probe \
                      object could not be removed code=\"kubernetes.api-failed\" \
                      name=\"sbx-pr-x-pvc\"\n";
        let since = before.len();
        let other = "2026-10-29T12:30:05Z  WARN substrate_daemon::app::service: workspace destroy \
                     reconciliation will retry workspace=ws_other code=\"kubernetes.other\"\n";
        let ours = "2026-10-29T12:30:06Z  WARN substrate_daemon::app::service: workspace destroy \
                    reconciliation will retry workspace=ws_one code=\"kubernetes.api-failed\"\n";
        assert!(pending_destroy_codes(before, since, "ws_one").is_empty());
        assert_eq!(
            pending_destroy_codes(&format!("{before}{other}{ours}"), since, "ws_one"),
            BTreeSet::from(["kubernetes.api-failed".to_owned()])
        );
        assert!(pending_destroy_codes(&format!("{before}{other}"), since, "ws_one").is_empty());
        let coloured = "\u{1b}[33m WARN\u{1b}[0m \u{1b}[2msubstrate_daemon\u{1b}[0m: workspace \
                        destroy reconciliation will retry \u{1b}[3mworkspace\u{1b}[0m\u{1b}[2m=\
                        \u{1b}[0mws_one \u{1b}[3mcode\u{1b}[0m\u{1b}[2m=\u{1b}[0m\
                        \"kubernetes.api-failed\"";
        assert_eq!(
            pending_destroy_codes(&format!("{before}{coloured}"), since, "ws_one"),
            BTreeSet::from(["kubernetes.api-failed".to_owned()])
        );
        assert!(
            pending_destroy_codes(&format!("{before}{coloured}"), since, "ws_on").is_empty(),
            "a prefix of the id is another workspace"
        );
    }

    #[test]
    fn a_probe_warn_names_the_fact_it_did_not_prove() {
        let log = "2026-10-29T10:02:34.861357Z  WARN substrate_kubernetes::namespace::probe: \
                   exec.argv-only was not proved why=\"the exec's log answered 403: pods \
                   \\\"sbx-pr-dw7dhppsb6lkqjib-pod\\\" is forbidden\"\n\
                   2026-10-29T10:02:35Z  INFO substrate_kubernetes: workspace.other was not proved";
        assert_eq!(
            unproved_facts(log),
            BTreeSet::from(["exec.argv-only".to_owned()])
        );
    }

    /// The line the probe loop writes once the first probe has finished and its facts stand.
    const PROBE_RAN: &str = "2026-11-01T01:00:03.000Z  INFO substrate_kubernetes::namespace::\
                             probe: the namespace driver's probe ran\n";

    /// The `WARN` line the driver writes when the API refuses `verb` on `resource`, the API's
    /// answer worded as the API server words it, in `why=`'s `Display` form (D339 U1) or, when
    /// `debug`, in the `Debug`-escaped form the driver wrote before it.
    fn refused_line(resource: &str, verb: &str, debug: bool) -> String {
        let answer = format!(
            "answered 403: {resource} \"sbx-pr-x\" is forbidden: User \
             \"system:serviceaccount:substrate:substrate-driver\" cannot {verb} resource \
             \"{resource}\" in API group \"\" in the namespace \"substrate\""
        );
        let why = if debug { format!("{answer:?}") } else { answer };
        format!(
            "2026-11-01T01:00:02.000Z  WARN substrate_kubernetes::namespace::probe: the probe \
             Pod's run proved nothing code=\"kubernetes.api-failed\" why={why}\n"
        )
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// D337 S6a, as observed: the lines the probe writes when the Role lacks `pods/log` `get`
    /// refuse the `pods/log` verb case, by the API's answer they carry once the probe has
    /// finished; without a finished probe, the same lines refuse nothing.
    #[test]
    fn a_refused_pod_log_read_refuses_the_verb_case() {
        let before = "2026-10-31T19:40:02.112Z  WARN substrate_kubernetes::namespace::probe: \
                      the probe Pod's run proved nothing why=\"the exec's log answered 403: pods \
                      \\\"sbx-pr-x-pod\\\" is forbidden: cannot get resource \\\"pods/log\\\"\"\n";
        let after = "2026-10-31T21:50:02.112Z  WARN substrate_kubernetes::namespace::probe: \
                     the probe Pod's run proved nothing code=\"kubernetes.api-failed\" \
                     why=\"the exec's log answered 403: pods \\\"sbx-pr-x-pod\\\" is forbidden: \
                     cannot get resource \\\"pods/log\\\"\"\n\
                     2026-10-31T21:50:02.113Z  WARN substrate_kubernetes::namespace::probe: \
                     confinement.family.process-run was not proved: the probe Pod's run proved \
                     nothing\n";
        let missing = set(&["confinement.family.process-run"]);
        assert!(
            verb_refusal(before, 0, "pods/log", "get", &missing).is_none(),
            "no probe has finished"
        );
        assert!(
            verb_refusal(after, 0, "pods/log", "get", &missing).is_none(),
            "no probe has finished"
        );
        let finished = format!("{after}{PROBE_RAN}");
        let refused = verb_refusal(&finished, 0, "pods/log", "get", &missing)
            .expect("the probe's answer refuses the case");
        assert!(
            refused.contains("cannot get resource \"pods/log\""),
            "{refused}"
        );
        for (resource, verb) in [("pods", "get"), ("pods/exec", "get"), ("pods/log", "list")] {
            assert!(
                verb_refusal(&finished, 0, resource, verb, &missing).is_none(),
                "the pods/log answer refuses {resource} {verb}"
            );
        }
    }

    /// The line the driver wrote at `bb2b06fe` when the Role lacked `pods/exec` `create`
    /// (2026-11-01 10:08:42Z, the daemon log the lane kept): the API's `Status.message` names
    /// the keeper Pod and ends at "is forbidden: ", with no reason. Since
    /// `story:driver-names-a-bare-exec-refusals-verb` the driver writes `reason` after it: what
    /// its access review found.
    fn bare_exec_line(pod: &str, code: &str, status: u16, reason: &str) -> String {
        format!(
            "2026-11-01T10:08:42.729145Z  WARN substrate_kubernetes::namespace::probe: the \
             namespace driver's probe failed; its facts are absent code=\"{code}\" the cluster \
             API: exec answered {status}: pods \"{pod}\" is forbidden: {reason}\n"
        )
    }

    /// What the driver writes after a bare refusal once its access review denies `pods/exec`
    /// `create` alone, and once it found anything else.
    const REVIEW_DENIES_CREATE: &str = "(a SelfSubjectAccessReview denies pods/exec create)";
    const REVIEW_DENIES_GET: &str = "(a SelfSubjectAccessReview denies pods/exec get)";
    const REVIEW_DENIES_BOTH: &str = "(a SelfSubjectAccessReview denies pods/exec create and get)";
    const REVIEW_UNDECIDED: &str = "(a SelfSubjectAccessReview admits pods/exec create and get: \
         it does not decide the refused verb)";
    const REVIEW_FAILED: &str = "(the refused verb is unknown: the SelfSubjectAccessReview of \
         pods/exec create failed: access review answered 403: \
         selfsubjectaccessreviews.authorization.k8s.io is forbidden: User \
         \"system:serviceaccount:snl-x:substrate-driver\" cannot create resource \
         \"selfsubjectaccessreviews\" in API group \"authorization.k8s.io\" at the cluster scope)";

    /// D349, then `story:driver-names-a-bare-exec-refusals-verb`: the "pods/exec create" case is
    /// refused by the API's bare refusal of the exec once the driver's access review names
    /// `create` alone as the verb it denies; the "pods/exec get" case stays strict, and neither
    /// case's line refuses the other.
    #[test]
    fn the_exec_create_case_reads_the_apis_bare_refusal_and_stays_distinct_from_get() {
        let missing = set(&["confinement.family.process-run", "exec.argv-only"]);
        let keeper = "sbx-pr-gbyb7pajquihb757-keeper";
        let handshake = "kubernetes.executor-handshake-failed";
        let bare = bare_exec_line(keeper, handshake, 403, REVIEW_DENIES_CREATE);
        let refused = verb_refusal(&bare, 0, "pods/exec", "create", &missing).expect(
            "the bare refusal whose review denies create refuses the create case \
             (story:driver-names-a-bare-exec-refusals-verb)",
        );
        assert!(refused.contains(keeper), "{refused}");
        assert!(refused.contains(REVIEW_DENIES_CREATE), "{refused}");
        let escaped = format!(
            "2026-11-01T10:08:42.729145Z  WARN substrate_kubernetes::namespace::probe: the \
             namespace driver's probe failed; its facts are absent \
             code=\"{handshake}\" message=\"the cluster API: exec answered 403: pods \
             \\\"{keeper}\\\" is forbidden: {REVIEW_DENIES_CREATE}\"\n"
        );
        assert!(
            verb_refusal(&escaped, 0, "pods/exec", "create", &missing).is_some(),
            "the Debug-escaped line refuses nothing: {escaped}"
        );
        assert!(
            verb_refusal(&bare, 0, "pods/exec", "get", &missing).is_none(),
            "the bare refusal refuses the get case"
        );
        for (resource, verb) in [("pods", "create"), ("pods", "get"), ("pods/log", "get")] {
            assert!(
                verb_refusal(&bare, 0, resource, verb, &missing).is_none(),
                "the bare refusal refuses {resource} {verb}"
            );
        }
        let get = bare_exec_line(
            "sbx-pr-sztvbtqqus5bw7uy-keeper",
            handshake,
            403,
            "User \"system:serviceaccount:snl-x:substrate-driver\" cannot get resource \
             \"pods/exec\" in API group \"\" in the namespace \"snl-x\"",
        );
        assert!(verb_refusal(&get, 0, "pods/exec", "get", &missing).is_some());
        assert!(
            verb_refusal(&get, 0, "pods/exec", "create", &missing).is_none(),
            "the get case's refusal refuses the create case"
        );
        assert!(
            verb_refusal(&bare, bare.len(), "pods/exec", "create", &missing).is_none(),
            "a bare refusal before the Role change refuses nothing"
        );
        assert!(
            verb_refusal(&bare, 0, "pods/exec", "create", &BTreeSet::new()).is_none(),
            "with every fact published nothing is refused"
        );
    }

    /// `story:driver-names-a-bare-exec-refusals-verb`: no line but the driver's bare refusal of
    /// the keeper's exec under the handshake code, followed by nothing but the review naming
    /// `create` alone, refuses the "pods/exec create" case.
    #[test]
    fn the_exec_create_case_refuses_on_no_line_but_the_drivers_naming_create_alone() {
        let missing = set(&["confinement.family.process-run", "exec.argv-only"]);
        let keeper = "sbx-pr-gbyb7pajquihb757-keeper";
        let handshake = "kubernetes.executor-handshake-failed";
        let bare = bare_exec_line(keeper, handshake, 403, REVIEW_DENIES_CREATE);
        for (line, what) in [
            (
                bare_exec_line(
                    "sbx-pr-gbyb7pajquihb757-pod",
                    handshake,
                    403,
                    REVIEW_DENIES_CREATE,
                ),
                "another Pod",
            ),
            (
                bare_exec_line(keeper, "kubernetes.api-failed", 403, REVIEW_DENIES_CREATE),
                "another code",
            ),
            (
                bare_exec_line(keeper, handshake, 404, REVIEW_DENIES_CREATE),
                "another status",
            ),
            (
                bare_exec_line(keeper, handshake, 403, "cannot list resource \"pods\""),
                "another verb's refusal",
            ),
            (
                bare_exec_line(keeper, handshake, 403, "the keeper is not running"),
                "a reason",
            ),
            (
                bare_exec_line(
                    keeper,
                    handshake,
                    403,
                    &format!("{REVIEW_DENIES_CREATE} the keeper is not running"),
                ),
                "a reason after the review",
            ),
            (
                bare_exec_line(keeper, handshake, 403, ""),
                "a bare refusal naming no verb, which D349 accepted until the driver named one \
                 (story:driver-names-a-bare-exec-refusals-verb)",
            ),
            (
                bare_exec_line(keeper, handshake, 403, REVIEW_DENIES_GET),
                "a review denying get, the get case's verb",
            ),
            (
                bare_exec_line(keeper, handshake, 403, REVIEW_DENIES_BOTH),
                "a review denying both verbs",
            ),
            (
                bare_exec_line(keeper, handshake, 403, REVIEW_UNDECIDED),
                "a review that did not decide",
            ),
            (
                bare_exec_line(keeper, handshake, 403, REVIEW_FAILED),
                "a failed review",
            ),
            (bare.replace(" WARN ", " INFO "), "an INFO line"),
        ] {
            assert!(
                verb_refusal(&line, 0, "pods/exec", "create", &missing).is_none(),
                "{what} refuses the create case: {line}"
            );
        }
    }

    /// Acceptance 1: a refusal written before `since`, the Role change, refuses nothing, even
    /// with the probe finished after it.
    #[test]
    fn a_refusal_written_before_the_role_change_refuses_nothing() {
        let missing = set(&["workspace.guarded-io"]);
        let earlier = format!("{}{PROBE_RAN}", refused_line("pods", "create", false));
        assert!(verb_refusal(&earlier, 0, "pods", "create", &missing).is_some());
        assert!(verb_refusal(&earlier, earlier.len(), "pods", "create", &missing).is_none());
        let refusal_before = refused_line("pods", "create", false);
        let log = format!("{refusal_before}{PROBE_RAN}");
        assert!(
            verb_refusal(&log, refusal_before.len(), "pods", "create", &missing).is_none(),
            "the finished probe alone, after the Role change, carries no refusal"
        );
    }

    /// Acceptance 2: a failed run whose line carries another resource's or another verb's
    /// refusal, a code alone, or a fact's "was not proved" line refuses nothing; `pods` never
    /// matches `pods/log` or `pods/exec`.
    #[test]
    fn a_refusal_naming_another_resource_or_verb_or_none_refuses_nothing() {
        let missing = set(&["exec.argv-only", "workspace.guarded-io"]);
        for (resource, verb) in [
            ("pods/log", "get"),
            ("pods/exec", "get"),
            ("pods/exec", "create"),
            ("pods", "list"),
            ("persistentvolumeclaims", "get"),
        ] {
            for debug in [false, true] {
                let log = format!("{}{PROBE_RAN}", refused_line(resource, verb, debug));
                assert!(
                    verb_refusal(&log, 0, "pods", "get", &missing).is_none(),
                    "{resource} {verb} (debug {debug}) refuses pods get"
                );
            }
        }
        let unrelated = format!(
            "2026-11-01T01:00:02.000Z  WARN substrate_kubernetes::namespace::probe: the probe \
             Pod's run proved nothing code=\"kubernetes.api-failed\" why=the exec's log was not \
             read within 60 s\n\
             2026-11-01T01:00:02.001Z  WARN substrate_kubernetes::namespace::probe: \
             exec.argv-only was not proved: the probe Pod's run proved nothing\n{PROBE_RAN}"
        );
        assert!(
            verb_refusal(&unrelated, 0, "pods", "get", &missing).is_none(),
            "a code and an unproved fact without the API's answer refuse nothing"
        );
        let info = format!(
            "{}{PROBE_RAN}",
            refused_line("pods", "get", false).replace(" WARN ", " INFO ")
        );
        assert!(
            verb_refusal(&info, 0, "pods", "get", &missing).is_none(),
            "only a WARN line refuses"
        );
        let whole = format!("{}{PROBE_RAN}", refused_line("pods", "get", false));
        assert!(
            verb_refusal(&whole, 0, "pods", "get", &set(&[])).is_none(),
            "no fact is missing"
        );
    }

    /// D339: the case decides only after the first probe has finished: the probe loop's own
    /// line that it ran or failed, or the open's refusal, which carries the API's answer and
    /// after which no probe runs.
    #[test]
    fn a_refusal_counts_only_once_the_first_probe_finished_or_the_open_was_refused() {
        let missing = set(&["workspace.guarded-io"]);
        let refusal = refused_line("pods", "create", false);
        assert!(verb_refusal(&refusal, 0, "pods", "create", &missing).is_none());
        let failed = format!(
            "{refusal}2026-11-01T01:00:03.000Z  WARN substrate_kubernetes::namespace::probe: the \
             namespace driver's probe failed; its facts are absent \
             code=\"kubernetes.api-failed\" message=pods create answered 403\n"
        );
        assert!(verb_refusal(&failed, 0, "pods", "create", &missing).is_some());
        let not_open = "2026-11-01T01:00:01.000Z  WARN substrate_kubernetes::namespace::probe: \
                        the namespace driver is not open; every driver fact is absent \
                        code=\"kubernetes.api-failed\" message=get answered 403: leases \
                        \"b10x-substrate-driver\" is forbidden: User \"system:serviceaccount:x:y\" \
                        cannot get resource \"leases\" in API group \"coordination.k8s.io\"\n";
        assert!(verb_refusal(not_open, 0, "leases", "get", &missing).is_some());
        assert!(
            verb_refusal(not_open, 0, "leases", "update", &missing).is_none(),
            "the open's refusal names another verb"
        );
        let other_not_open = format!(
            "{}{refusal}",
            not_open.replace(
                "cannot get resource \"leases\"",
                "cannot get resource \"x\""
            )
        );
        assert!(
            verb_refusal(&other_not_open, 0, "pods", "create", &missing).is_none(),
            "an open refused for another reason has not finished a probe"
        );
    }

    /// Each Role-verb case the lane runs has a refusal rule that can be met: the driver's line
    /// carrying the API's answer for its own verb, which refuses no other case; `pods list` and
    /// `persistentvolumeclaims delete` keep the route's HTTP answer besides.
    #[test]
    fn every_run_role_verb_case_has_a_refusal_rule_that_can_be_met() {
        let run: Vec<(String, String)> = role_verbs()
            .into_iter()
            .filter(|(resource, verb)| {
                !ABSENT
                    .iter()
                    .any(|(absent, _)| *absent == verb_case(resource, verb))
            })
            .collect();
        assert_eq!(run.len(), 14, "{run:?}");
        let names = cases();
        for (resource, verb) in &run {
            assert!(names.contains(&verb_case(resource, verb)));
        }
        for route in [("pods", "list"), ("persistentvolumeclaims", "delete")] {
            assert!(
                run.contains(&(route.0.to_owned(), route.1.to_owned())),
                "{route:?} is a run case whose route answers"
            );
        }
        let missing = set(&["workspace.guarded-io"]);
        for (resource, verb) in &run {
            for debug in [false, true] {
                let log = format!("{}{PROBE_RAN}", refused_line(resource, verb, debug));
                assert!(
                    verb_refusal(&log, 0, resource, verb, &missing).is_some(),
                    "{resource} {verb} (debug {debug}) is not refused by its own answer"
                );
                for (other_resource, other_verb) in &run {
                    if (other_resource, other_verb) != (resource, verb) {
                        assert!(
                            verb_refusal(&log, 0, other_resource, other_verb, &missing).is_none(),
                            "{resource} {verb}'s answer refuses {other_resource} {other_verb}"
                        );
                    }
                }
            }
        }
    }

    /// The lines the lane reads to know the first probe finished are the driver's own words.
    #[test]
    fn the_driver_writes_the_lines_that_finish_a_probe() {
        let source = include_str!("../../crates/substrate-kubernetes/src/namespace/probe.rs");
        for line in PROBE_FINISHED.iter().chain([&NOT_OPEN]) {
            assert!(
                source.contains(line),
                "the driver no longer writes {line:?}"
            );
        }
    }

    #[test]
    fn the_driver_kubeconfig_carries_the_extension_on_its_context() {
        let admin = json!({"contexts": [{"name": "lane", "context": {"cluster": "lane"}}]});
        let settings = Settings {
            executor_image: "localhost:1/e@sha256:aa".to_owned(),
            exec_image: "localhost:1/b@sha256:bb".to_owned(),
            egress_probe_address: "172.18.0.1:9".to_owned(),
        };
        let config = with_extension(admin, &settings);
        let extension = &config["contexts"][0]["context"]["extensions"][0];
        assert_eq!(extension["name"], EXTENSION);
        assert_eq!(
            extension["extension"]["executor_image"],
            "localhost:1/e@sha256:aa"
        );
        assert_eq!(
            extension["extension"]["default_deny_policy"],
            render::DEFAULT_DENY
        );
        assert_eq!(extension["extension"]["probe_interval_seconds"], 3_600);
    }

    #[test]
    fn only_the_claim_may_remain_and_the_inventory_must_match_exactly() {
        assert!(unexpected_leftovers(&[format!("Lease/{LEASE}")]).is_empty());
        assert_eq!(
            unexpected_leftovers(&[format!("Lease/{LEASE}"), "Pod/sbx-ws-a-keeper".to_owned()]),
            ["Pod/sbx-ws-a-keeper"]
        );
        let before =
            Inventory::from([("NamespaceList".to_owned(), BTreeSet::from(["a".to_owned()]))]);
        let mut after = before.clone();
        assert!(inventory_difference(&before, &after).is_empty());
        after
            .entry("PersistentVolumeList".to_owned())
            .or_default()
            .insert("pvc-1".to_owned());
        assert_eq!(
            inventory_difference(&before, &after),
            ["PersistentVolumeList gained pvc-1"]
        );
    }

    /// The runner's subject as the namespace daemon serves it: a destroy the driver leaves to
    /// reconciliation answers `operation.outcome-unknown` and reads absent a few reads later; a
    /// second destroy of a `destroying` workspace is refused `workspace.not-ready`.
    #[derive(Default)]
    struct Subject {
        /// Each workspace's state and the reads left until it reads absent once destroying.
        workspaces: BTreeMap<String, (String, u32)>,
        deletes: Vec<String>,
        /// A destroy of this workspace is refused with this status and code.
        refuse: Option<(String, u16, &'static str)>,
        /// This workspace never reads absent.
        stuck: Option<String>,
    }

    impl Subject {
        fn holding(states: &[(&str, &str)]) -> Self {
            Self {
                workspaces: states
                    .iter()
                    .map(|(id, state)| ((*id).to_owned(), ((*state).to_owned(), 2)))
                    .collect(),
                ..Self::default()
            }
        }

        fn answer(&mut self, method: &str, path: &str) -> (u16, Value) {
            let (route, query) = path.split_once('?').unwrap_or((path, ""));
            let failure = |status, code: &str| (status, json!({"error": {"code": code}}));
            let segments: Vec<&str> = route.trim_start_matches("/v1/").split('/').collect();
            match (method, segments.as_slice()) {
                ("POST", ["reconciliation-snapshots"]) => {
                    (201, json!({"result": {"id": "snap_1"}}))
                }
                ("GET", ["reconciliation-snapshots", "snap_1"]) => {
                    // One workspace per page, so the sweep must follow the cursor.
                    let after: usize = query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("cursor=c"))
                        .and_then(|seq| seq.parse().ok())
                        .unwrap_or(0);
                    let items: Vec<Value> = self
                        .workspaces
                        .iter()
                        .skip(after)
                        .take(1)
                        .map(|(id, (state, _))| {
                            json!({"kind": "workspace", "id": id, "value": {"state": state}})
                        })
                        .collect();
                    let next =
                        (after + 1 < self.workspaces.len()).then(|| format!("c{}", after + 1));
                    (
                        200,
                        json!({"result": {"items": items, "next_cursor": next}}),
                    )
                }
                ("DELETE", ["workspaces", id]) => {
                    let id = *id;
                    self.deletes.push(id.to_owned());
                    if let Some((refused, status, code)) = &self.refuse
                        && refused == id
                    {
                        return failure(*status, code);
                    }
                    match self.workspaces.get_mut(id) {
                        None => failure(404, "resource.not-found"),
                        Some((state, _)) if state == "destroying" => {
                            failure(409, "workspace.not-ready")
                        }
                        Some((state, _)) => {
                            "destroying".clone_into(state);
                            failure(500, OUTCOME_UNKNOWN)
                        }
                    }
                }
                ("GET", ["workspaces", id]) => {
                    let id = *id;
                    let stuck = self.stuck.as_deref() == Some(id);
                    match self.workspaces.get_mut(id) {
                        None => failure(404, "resource.not-found"),
                        Some((state, reads)) if state == "destroying" && !stuck => {
                            if *reads == 0 {
                                self.workspaces.remove(id);
                                return failure(404, "resource.not-found");
                            }
                            *reads -= 1;
                            (200, json!({"result": {"id": id, "state": "destroying"}}))
                        }
                        Some((state, _)) => (200, json!({"result": {"id": id, "state": state}})),
                    }
                }
                _ => failure(404, "resource.not-found"),
            }
        }
    }

    fn sweep(subject: &mut Subject, bound: Duration) -> Result<Vec<String>> {
        sweep_workspaces(
            &mut |method, path, _| Ok(subject.answer(method, path)),
            bound,
            Duration::ZERO,
        )
    }

    /// story:clean-room-destroy-settles-on-the-namespace-driver: step 4 leaves no workspace of
    /// its own behind. Each one the subject holds is destroyed once and waited out until it reads
    /// absent; one already `destroying` is only waited out (design 01 § 6.2: never a second
    /// destroy under a fresh id).
    #[test]
    fn step_four_destroys_every_workspace_its_subject_holds_and_waits_until_each_is_absent() {
        let mut subject =
            Subject::holding(&[("ws_a", "ready"), ("ws_b", "ready"), ("ws_c", "destroying")]);
        let swept = sweep(&mut subject, Duration::from_secs(5)).expect("the sweep settles");
        assert!(
            subject.workspaces.is_empty(),
            "step 4 left {:?} for steps 5 and 6",
            subject.workspaces.keys().collect::<Vec<_>>()
        );
        assert_eq!(swept, ["ws_a", "ws_b", "ws_c"]);
        assert_eq!(
            subject.deletes,
            ["ws_a", "ws_b"],
            "each ready workspace is destroyed once, and the destroying one not at all"
        );
    }

    /// A destroy refused for another reason, and a workspace that never reads absent, fail the
    /// sweep by name instead of passing a step that left a workspace behind.
    #[test]
    fn a_refused_destroy_or_a_workspace_that_stays_fails_the_sweep_by_name() {
        let mut refused = Subject::holding(&[("ws_a", "ready")]);
        refused.refuse = Some(("ws_a".to_owned(), 403, "auth.forbidden"));
        let error = format!(
            "{:#}",
            sweep(&mut refused, Duration::from_secs(5)).expect_err("a refusal fails")
        );
        assert!(
            error.contains("ws_a") && error.contains("auth.forbidden"),
            "{error}"
        );
        let mut stuck = Subject::holding(&[("ws_a", "ready")]);
        stuck.stuck = Some("ws_a".to_owned());
        let error = format!(
            "{:#}",
            sweep(&mut stuck, Duration::from_millis(50)).expect_err("a stuck one fails")
        );
        assert!(error.contains("ws_a"), "{error}");
    }

    /// D342: a failed case keeps the daemon logs written while it ran, outside the run's
    /// temporary directory, and its failure names where; a log no daemon wrote during the case,
    /// and a file that is no daemon log, are not kept.
    #[test]
    fn a_failed_case_keeps_the_daemon_logs_written_while_it_ran_and_names_them() {
        let work = tempfile::tempdir().expect("tempdir");
        let kept = tempfile::tempdir().expect("tempdir");
        let verbs = work.path().join("verbs");
        let lane = work.path().join("lane");
        fs::create_dir_all(&verbs).expect("verbs");
        fs::create_dir_all(&lane).expect("lane");
        let earlier = lane.join("daemon-01JA0000000000000000000000.log");
        fs::write(&earlier, "an earlier case's daemon\n").expect("written");
        fs::File::options()
            .write(true)
            .open(&earlier)
            .and_then(|file| {
                file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
            })
            .expect("aged");
        let since = SystemTime::now();
        let current = verbs.join("daemon-01JB0000000000000000000000.log");
        fs::write(
            &current,
            "WARN code=\"kubernetes.executor-handshake-failed\"\n",
        )
        .expect("written");
        fs::write(verbs.join("state.db"), "not a log").expect("written");

        let failed = kept_case(
            work.path(),
            kept.path(),
            "S6 Role without pods/exec create",
            since,
            Case::Failed("nothing refused within 120 s".to_owned()),
        );
        let Case::Failed(error) = failed else {
            panic!("the case stays failed: {failed:?}");
        };
        let copy = kept
            .path()
            .join("s6-role-without-pods-exec-create")
            .join("daemon-01JB0000000000000000000000.log");
        assert!(
            error.starts_with("nothing refused within 120 s; daemon log kept at ")
                && error.contains(&copy.display().to_string()),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(&copy).expect("kept"),
            fs::read_to_string(&current).expect("read")
        );
        let case_directory = kept.path().join("s6-role-without-pods-exec-create");
        let names: Vec<String> = fs::read_dir(&case_directory)
            .expect("listed")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["daemon-01JB0000000000000000000000.log"]);
        assert_eq!(
            fs::metadata(&case_directory)
                .expect("kept")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let passed = kept_case(
            work.path(),
            kept.path(),
            "S7 everything destroyed through the daemon",
            since,
            Case::Passed("destroyed".to_owned()),
        );
        assert_eq!(passed, Case::Passed("destroyed".to_owned()));
        assert!(
            !kept
                .path()
                .join("s7-everything-destroyed-through-the-daemon")
                .exists(),
            "a passed case keeps nothing"
        );
    }

    /// Adversary case for D342 U1. The verb cases run back to back in one directory: case N-1's
    /// daemon is stopped inside `verb`, then `set_role(None)` runs, then N-1 is recorded and `since`
    /// is taken, so N-1's log was last written a few hundred ms before case N began. A failed
    /// case N keeps the logs written while it ran, and N-1's is not one of them.
    #[test]
    fn a_log_last_written_before_the_case_began_is_not_kept_with_it() {
        let work = tempfile::tempdir().expect("tempdir");
        let kept = tempfile::tempdir().expect("tempdir");
        let verbs = work.path().join("verbs");
        fs::create_dir_all(&verbs).expect("verbs");
        let since = SystemTime::now();
        let previous = verbs.join("daemon-01JA0000000000000000000000.log");
        fs::write(&previous, "the previous verb case's daemon, stopped\n").expect("written");
        fs::File::options()
            .write(true)
            .open(&previous)
            .and_then(|file| file.set_modified(since - Duration::from_millis(300)))
            .expect("aged");
        let current = verbs.join("daemon-01JB0000000000000000000000.log");
        fs::write(
            &current,
            "WARN code=\"kubernetes.executor-handshake-failed\"\n",
        )
        .expect("written");

        let logs = keep_daemon_logs(
            work.path(),
            kept.path(),
            "S6 Role without pods/exec create",
            since,
        )
        .expect("kept");
        let names: Vec<String> = logs
            .iter()
            .filter_map(|log| log.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["daemon-01JB0000000000000000000000.log"],
            "a log the previous case's daemon wrote before this case began is kept and named \
             as this case's"
        );
    }

    /// Adversary cases of wave D354 (`story:driver-names-a-bare-exec-refusals-verb`).
    mod d354_adv {
        /// The S6 create case's line is the driver's own words: the lane's
        /// `REVIEW_DENIES_CREATE` is `ACCESS_REVIEW_DENIES` in the driver with `create` alone,
        /// in parentheses, and the driver's own test pins that composed line on the probe's
        /// refusal. A driver that rewords it fails here, not first on the live lane.
        #[test]
        fn the_create_cases_review_line_is_the_drivers_own_words() {
            let workspace =
                include_str!("../../crates/substrate-kubernetes/src/namespace/workspace.rs");
            let pinned = include_str!(
                "../../crates/substrate-kubernetes/src/namespace/verb_review_tests.rs"
            );
            let declared = "const ACCESS_REVIEW_DENIES: &str = \"";
            let start = workspace
                .find(declared)
                .map(|at| at + declared.len())
                .expect("the driver declares ACCESS_REVIEW_DENIES");
            let denies =
                &workspace[start..][..workspace[start..].find('"').expect("a closed literal")];
            assert_eq!(
                super::super::REVIEW_DENIES_CREATE,
                format!("({denies}create)"),
                "story:driver-names-a-bare-exec-refusals-verb"
            );
            assert!(
                workspace.contains("const EXEC_VERBS: [&str; 2] = [\"create\", \"get\"];"),
                "story:driver-names-a-bare-exec-refusals-verb: the driver reviews create first"
            );
            assert!(
                pinned.contains(&format!("{{BARE}} {}", super::super::REVIEW_DENIES_CREATE)),
                "story:driver-names-a-bare-exec-refusals-verb: the driver's test no longer pins \
                 the line the lane reads"
            );
        }
    }
}
