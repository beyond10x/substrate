//! Design 34 § 10.2 steps 3 to 7 of `cargo xtask namespace-lane observe` (§ 12 U13): the daemon
//! with `--driver kubernetes` on the workstation, the vectors' namespace lane, the driver-specific
//! cases of ADR 0047 decision 13 and the cleanup assertions.
//!
//! The daemon listens on its Unix socket, not over TLS: the chart lane's Identity stub runs inside
//! its cluster's daemon Pods (`xtask/src/chart_cluster.rs`) and offers nothing a workstation
//! process can reuse. Every daemon the lane starts is the shipped binary, built from this checkout;
//! the lane talks to it over its socket and to the cluster with the admin credential only to set a
//! case up, to look at what the driver left and to release the namespace's Lease once the daemon
//! that held it is stopped: the Role grants the driver no `delete` on Leases (decision 3), so a
//! stopped daemon's claim would hold the namespace for 30 s, or for ever for another deployment.
//!
//! Step 4, design 31's clean-room runner, does not exist in this wave (D186): it is named in
//! [`ABSENT`] and never counted. A case the driver cannot reach on this wave is named there too,
//! with its reason; every other case in [`cases`] must pass once.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use substrate_kubernetes_client::{Api, ListForm};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;

use crate::chart_cluster::output;
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
/// When a Role-verb case whose facts stayed whole tries its dependent route.
const VERB_ROUTE_AFTER: Duration = Duration::from_secs(5);
/// How long the privileged dry-run case waits for its refusal.
const REFUSAL_BOUND: Duration = Duration::from_secs(90);
/// How long the cluster-scoped inventory may take to settle back: dynamic PVs go after their
/// claims.
const INVENTORY_BOUND: Duration = Duration::from_mins(3);

/// The step 4 reason (D186).
pub const STEP_FOUR_ABSENT: &str = "postponed to story:remote-clean-room-conformance (D186): \
                                    design 31's clean-room runner does not exist in this wave";
/// Why every exec-dependent case is absent on this wave.
pub const NO_EXEC: &str = "no exec is admitted on this wave: the driver never publishes \
                           confinement.no-egress (O3 did not hold on kind), so every exec start \
                           is refused exec.sandbox-unavailable before a Pod exists";

/// The cases named absent, with their reasons; none is ever counted as passed.
pub const ABSENT: [(&str, &str); 7] = [
    ("S4 clean-room runner", STEP_FOUR_ABSENT),
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

/// A Role-verb case's refusal once a full-set fact is `missing`: ADR 0047 decision 13's "the
/// dependent fact is absent and its refusal named". A reason code on a `WARN` line names it, or
/// the probe's `WARN` that this very missing fact was not proved, which states why.
pub fn verb_refusal(
    missing: &BTreeSet<String>,
    codes: &BTreeSet<String>,
    unproved: &BTreeSet<String>,
) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    if !codes.is_empty() {
        return Some(format!("absent {missing:?}; the log names {codes:?}"));
    }
    let stated: Vec<&String> = missing.intersection(unproved).collect();
    (!stated.is_empty()).then(|| {
        let warned: Vec<String> = stated
            .iter()
            .map(|fact| format!("{fact} was not proved"))
            .collect();
        format!("absent {missing:?}; the log warns {warned:?}, with the API's answer")
    })
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
        let deadline = Instant::now() + Duration::from_secs(30);
        while !daemon.socket.exists() {
            if let Some(child) = daemon.child.as_mut()
                && let Some(status) = child.try_wait()?
            {
                bail!(
                    "the daemon exited before its socket existed ({status}): {}",
                    daemon.log()
                );
            }
            ensure!(
                Instant::now() < deadline,
                "the daemon created no socket within 30 s: {}",
                daemon.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(daemon)
    }

    /// Everything the daemon wrote to stderr so far.
    pub fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
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
        let mut record = |name: &str, case: Case| {
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

    /// One Role verb removed: a fact the whole Role publishes is absent and the log names a
    /// reason code, or the route that needs the verb is refused by name.
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
            let facts = published(&daemon.machine().await?);
            let log = daemon.log();
            let codes = named_codes(&log);
            let missing: BTreeSet<String> = full.difference(&facts).cloned().collect();
            if started.elapsed() > Duration::from_secs(2)
                && let Some(refused) = verb_refusal(&missing, &codes, &unproved_facts(&log))
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
                    "nothing refused within {} s: facts {facts:?}, codes {codes:?}, route {route:?}",
                    VERB_BOUND.as_secs()
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
        let before = named_codes(&daemon.log());
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
                    let named: Vec<String> = named_codes(&daemon.log())
                        .difference(&before)
                        .cloned()
                        .collect();
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

    #[test]
    fn every_role_verb_of_decision_three_is_a_case_and_step_four_is_absent_by_name() {
        let names = cases();
        assert_eq!(role_verbs().len(), 17, "decision 3 grants 17 verbs");
        for (resource, verb) in role_verbs() {
            assert!(
                names.contains(&verb_case(&resource, &verb)),
                "{resource} {verb}"
            );
        }
        assert!(ABSENT.contains(&("S4 clean-room runner", STEP_FOUR_ABSENT)));
        assert!(STEP_FOUR_ABSENT.contains("postponed to story:remote-clean-room-conformance"));
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

    #[test]
    fn an_absent_case_reported_passed_a_failure_or_a_missing_case_is_no_record() {
        let mut passed_absent = passing();
        let step_four = passed_absent
            .iter_mut()
            .find(|(name, _)| name == "S4 clean-room runner")
            .expect("step 4");
        step_four.1 = Case::Passed("ran".to_owned());
        assert!(verdict(&passed_absent).is_some_and(|fault| fault.contains("S4")));

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

    #[test]
    fn a_missing_fact_is_refused_by_a_code_or_by_its_own_unproved_warn() {
        let set = |names: &[&str]| -> BTreeSet<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        let by_warn = verb_refusal(
            &set(&["exec.argv-only"]),
            &set(&[]),
            &set(&["exec.argv-only"]),
        )
        .expect("the probe's WARN names the missing fact");
        assert!(
            by_warn.contains("exec.argv-only was not proved"),
            "{by_warn}"
        );
        assert!(
            verb_refusal(
                &set(&["exec.argv-only"]),
                &set(&[]),
                &set(&["workspace.guarded-io"])
            )
            .is_none()
        );
        assert!(verb_refusal(&set(&[]), &set(&["kubernetes.api-failed"]), &set(&[])).is_none());
        let by_code = verb_refusal(
            &set(&["workspace.guarded-io"]),
            &set(&["kubernetes.api-failed"]),
            &set(&[]),
        )
        .expect("a code names the refusal");
        assert!(by_code.contains("kubernetes.api-failed"), "{by_code}");
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
}
