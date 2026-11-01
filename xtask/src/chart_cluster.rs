//! `cargo xtask chart-lane cluster`: ADR 0046's install, upgrade and uninstall scenarios on kind.
//!
//! The verb creates its own kind cluster (one control plane, two workers it labels and taints as
//! the dedicated pool), loads the daemon image built from the checkout, and always deletes the
//! cluster again. Identity is a stub on the runner, reachable from the pods at the kind network's
//! IPv4 gateway, with a certificate for that address; the daemon's own certificate names exactly
//! the per-pod hosts the chart's `certificate-names` `ConfigMap` computes (K2 A). The scenarios:
//!
//! 1. every daemon pod is ready, each on a different worker of the pool;
//! 2. each pod's certificate verifies under its own host name and fails under the Service's;
//! 3. design 31's clean-room runner, exported from the `Dockerfile`'s `conformance-runner` stage,
//!    certifies ordinal 0 over a port-forward with verdict `passed`; its absent cases go to the
//!    job summary unpinned and its report is the workflow's artifact (design 31 § 7);
//! 4. a workspace, a file in it and an operation replay survive a rolling restart by
//!    `helm upgrade`, and the replayed operation answers the original workspace;
//! 5. the daemon dials a declared Git source and its connection to an undeclared port on the same
//!    host never arrives (the `NetworkPolicy`); if it did not dial the declared one either, the
//!    scenario is reported absent, never passed;
//! 6. the execution-enabled pod is admitted by server dry run in a `privileged` namespace and
//!    refused in a `baseline` one, and the execution-off install itself runs under `restricted`;
//! 7. `helm uninstall` leaves exactly the ledger claims.
//!
//! The hosted lane reports the four execution-enabled scenarios absent, by name. With
//! `--node-profiles` the verb is ADR 0046's K9 C: a workstation lane on one kind worker, because
//! every kind node shares the host kernel's `AppArmor` profiles, built from the committed
//! `fixtures/chart-lab/Dockerfile` (the hosted node image plus the distribution's pinned
//! `apparmor_parser`) and given the host's securityfs. It refuses, naming the prerequisite and
//! every scenario that therefore did not run, an image whose revision label is not `HEAD`, a recipe
//! not built on `--node-image`, and a worker without `AppArmor`, without the parser, already
//! holding this run's own profile name in the shared kernel, or without a cgroup v2 hierarchy
//! carrying `cpu`, `memory` and `pids`. Each run names its profile
//! `substrate-host-exec-v2-lane-<ulid>`, fresh and lowercased (design 33 § 7), so it never loads,
//! selects or removes a profile it does not own, the default `substrate-host-exec-v2` included. It
//! installs the chart with one pod, `execution.enabled`, `execution.appArmorProfile` set to that
//! name and `nodeProfiles.enabled` in `privileged` namespaces and runs the four:
//!
//! 1. the bootstrap dropped to uid 65532 with no capability, and a confined exec runs through it;
//! 2. the daemon runs under seccomp with the Localhost profile the node holds byte for byte;
//! 3. the daemon and its init run under the run's own enforced `AppArmor` profile;
//! 4. the 0400 key is owned by 65532 and the daemon, holding no capability, serves TLS with it.
//!
//! It then uninstalls the release and removes the profile its own run loaded, by its name, through
//! the node, which holds the source, so the next run starts from the same kernel. It prints
//! `Chart-lane: <sha> <count> <instant>` only when every declared scenario passed, the profile was
//! removed, `HEAD` did not move and the checkout was clean before and after the run. It installs
//! the chart and compares the seccomp profile as that commit holds them, extracted with
//! `git archive`, as it builds the recipe that commit holds.
//!
//! The cluster is the run's, and deleted on every exit path, only once `kind create` succeeded. A
//! SIGINT or SIGTERM stops the running command and runs the same cleanup, the cluster's deletion
//! and the removal of the run's own profile, before the verb exits non-zero; a second signal exits
//! at once, and the commands that remove each by hand are printed when the run creates it.

use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use clap::Args as ClapArgs;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
use serde_json::{Value, json};

use crate::chart::{self, CHART};
use crate::chart_yaml;
use crate::lane_record::{git_text, record_line};

const RELEASE: &str = "lane";
const NAMESPACE: &str = "substrate-lane";
const PRIVILEGED_NAMESPACE: &str = "substrate-lane-privileged";
const NODE_PROFILES_NAMESPACE: &str = "substrate-lane-node-profiles";
const BASELINE_NAMESPACE: &str = "substrate-lane-baseline";
const POOL_KEY: &str = "b10x.dev/substrate-pool";
const POOL_VALUE: &str = "lane";
const REPLICAS: usize = 2;
/// Local mode's workers and daemon pods. Every kind node shares the host kernel and its `AppArmor`
/// profiles, so a second node's installer would find the first one's profile loaded without its
/// own source and refuse it.
const LOCAL_REPLICAS: usize = 1;
/// The checked-in recipe local mode builds its kind node image from, at the recorded commit.
const LAB_RECIPE: &str = "fixtures/chart-lab/Dockerfile";
/// The local, never pushed, tag of that image; one per checkout.
const LAB_REPOSITORY: &str = "b10x-substrate-chart-lab";
const AUDIENCE: &str = "urn:b10x:substrate";
const SELECTOR: &str = "app.kubernetes.io/instance=lane,app.kubernetes.io/component=daemon";
const REVISION_LABEL: &str = "org.opencontainers.image.revision";
const TRAILER: &str = "Chart-lane";
/// This file, relative to the workspace root: the release step counts its `SCENARIOS`.
const SOURCE: &str = "xtask/src/chart_cluster.rs";
/// The chart's default host execution profile; every name the lane chooses extends it.
const APPARMOR_PROFILE: &str = "substrate-host-exec-v2";
const SECCOMP_PROFILE: &str = "b10x-substrate/host-exec-v1-amd64.json";
const SECCOMP_SOURCE: &str = "deploy/seccomp/host-exec-v1-amd64.json";
const DAEMON_UID: &str = "65532";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// The daemon image, `repository:tag`, as built from this checkout; loaded into the cluster.
    #[arg(long)]
    image: String,
    /// The pinned kind node image the cluster runs.
    #[arg(long)]
    node_image: String,
    /// The `kind` executable.
    #[arg(long, default_value = "kind")]
    kind: PathBuf,
    /// The `kubectl` executable.
    #[arg(long, default_value = "kubectl")]
    kubectl: PathBuf,
    /// The `helm` executable.
    #[arg(long, default_value = "helm")]
    helm: PathBuf,
    /// The `docker` executable.
    #[arg(long, default_value = "docker")]
    docker: PathBuf,
    /// The kind cluster's name; the verb refuses one that already exists.
    #[arg(long, default_value = "substrate-chart-lane")]
    cluster: String,
    /// K9 C, the workstation lane: one worker built from `fixtures/chart-lab/Dockerfile` with the
    /// host's securityfs, the node profiles installed, the execution-enabled scenarios run for real
    /// and a `Chart-lane:` record printed.
    #[arg(long)]
    node_profiles: bool,
    /// Design 31's clean-room runner, as the workflow exports it from the `Dockerfile`'s
    /// `conformance-runner` stage; the hosted lane runs it once against ordinal 0.
    #[arg(long)]
    conformance_runner: Option<PathBuf>,
    /// The directory the runner writes its report into; it must not exist. By default it is in
    /// the lane's own work directory.
    #[arg(long)]
    conformance_out: Option<PathBuf>,
}

/// What one scenario came to.
#[derive(Debug)]
enum Outcome {
    Passed(String),
    Absent(String),
}

/// The execution-enabled scenarios of ADR 0046 the lane declares and never runs: kind's nodes load
/// no Substrate seccomp or `AppArmor` profile and grant no delegated execution, so each is reported
/// absent, by name, on every run (K9 C records them locally).
pub const EXECUTION_ABSENT: [(&str, &str); 4] = [
    (
        "execution bootstrap",
        "the execution bootstrap starting as root and dropping to uid 65532 is not run on kind",
    ),
    (
        "Localhost seccomp profile",
        "the daemon under the b10x-substrate/host-exec-v1-amd64.json Localhost seccomp profile is not run on kind",
    ),
    (
        "Localhost AppArmor profile",
        "the daemon under the substrate-host-exec-v2 Localhost AppArmor profile is not run on kind",
    ),
    (
        "TLS key after the uid drop",
        "reading the 0400 TLS key after the bootstrap drops to uid 65532 is not run on kind",
    ),
];

/// The scenarios the hosted mode runs, in order, on two workers.
const HOSTED: [&str; 7] = [
    "placement",
    "certificate names",
    "clean-room runner",
    "restart survival",
    "egress",
    "pod security",
    "uninstall",
];

/// The scenarios `--node-profiles` runs and a `Chart-lane:` record counts, one per line: exactly
/// the execution-enabled ones the hosted mode reports absent. `release.yml` counts its entries in
/// the tagged source by the rule of [`declared_scenarios`], so keep it one quoted name per line.
pub const SCENARIOS: [&str; 4] = [
    "execution bootstrap",
    "Localhost seccomp profile",
    "Localhost AppArmor profile",
    "TLS key after the uid drop",
];

/// The number of entries of the one `pub const SCENARIOS: [&str; N] = [` array in `source`, each
/// on its own line as `    "name",`, closed by `];` — the count `release.yml` reads from the
/// tagged source. `None` for a missing, repeated, empty or malformed array, or one whose declared
/// length is not its entry count, and for any source holding a carriage return: `lines()` strips
/// one before a line feed, the workflow's awk does not, so the two would count differently.
pub fn declared_scenarios(source: &str) -> Option<usize> {
    if source.contains('\r') {
        return None;
    }
    let mut lines = source.lines();
    let mut found = None;
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("pub const SCENARIOS: [&str; ") else {
            continue;
        };
        if found.is_some() {
            return None;
        }
        let declared: usize = rest.strip_suffix("] = [")?.parse().ok()?;
        let mut count = 0;
        let mut closed = false;
        for entry in lines.by_ref() {
            if entry == "];" {
                closed = true;
                break;
            }
            let name = entry.strip_prefix("    \"")?.strip_suffix("\",")?;
            if name.is_empty() || name.contains('"') {
                return None;
            }
            count += 1;
        }
        if !closed || count == 0 || count != declared {
            return None;
        }
        found = Some(count);
    }
    found
}

/// The declared count, when a local run produced exactly one passed outcome per declared scenario
/// and no failure; otherwise why the run is no record.
fn local_verdict(outcomes: &[(String, Outcome)], failures: &[String]) -> Result<usize, String> {
    if !failures.is_empty() {
        return Err(format!(
            "{} scenario(s) failed: {}",
            failures.len(),
            failures.join("; ")
        ));
    }
    let mut seen = BTreeSet::new();
    for (name, outcome) in outcomes {
        if !SCENARIOS.contains(&name.as_str()) {
            return Err(format!("{name} is not a declared scenario"));
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("{name} ran twice"));
        }
        if let Outcome::Absent(reason) = outcome {
            return Err(format!("{name} is absent, not passed: {reason}"));
        }
    }
    let missing: Vec<&str> = SCENARIOS
        .iter()
        .copied()
        .filter(|name| !seen.contains(name))
        .collect();
    if missing.is_empty() {
        Ok(SCENARIOS.len())
    } else {
        Err(format!(
            "these declared scenarios did not run: {}",
            missing.join("; ")
        ))
    }
}

/// What a finished local run prints.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Every scenario passed at an unmoved, clean `HEAD`: print the record.
    Record(usize),
    /// Every scenario passed, but the run is no record for any commit.
    NoRecord(String),
    /// A scenario failed, was absent or did not run.
    Failed(String),
}

fn decision(
    verdict: Result<usize, String>,
    sha: &str,
    head_after: &str,
    clean_before: bool,
    clean_after: bool,
) -> Decision {
    match verdict {
        Err(reason) => Decision::Failed(reason),
        Ok(_) if head_after != sha => Decision::NoRecord(format!(
            "HEAD moved from {sha} to {head_after} during the run"
        )),
        Ok(count) if clean_before && clean_after => Decision::Record(count),
        Ok(_) => Decision::NoRecord(format!(
            "the checkout was not clean {} the run",
            if clean_before { "after" } else { "before" }
        )),
    }
}

/// The refusal for a missing prerequisite: what is missing, and every scenario that did not run.
fn refusal(prerequisite: &str) -> String {
    format!(
        "chart lane cluster: refusing, no record: {prerequisite}. None of the {} declared scenarios \
         ran: {}",
        SCENARIOS.len(),
        SCENARIOS.join("; ")
    )
}

/// The image must name the commit the record will name.
fn revision_verdict(label: &str, sha: &str) -> Result<(), String> {
    if label == sha {
        Ok(())
    } else {
        Err(format!(
            "the image's {REVISION_LABEL} label is {label:?}, not HEAD {sha}; build it from this \
             commit with --build-arg SOURCE_SHA={sha}"
        ))
    }
}

fn evidence_line(sha: &str, count: usize, at: SystemTime) -> String {
    record_line(TRAILER, sha, count, at)
}

/// The image the chart lab's recipe builds on: its one `FROM` reference. Docker reads the
/// instruction word in any case, after leading white space, and separated by a space or a tab, so
/// every such spelling counts as a stage.
fn lab_base(recipe: &str) -> Option<&str> {
    let mut bases = recipe.lines().filter_map(|line| {
        let (word, rest) = line
            .trim_start()
            .split_once(|character: char| character.is_ascii_whitespace())?;
        word.eq_ignore_ascii_case("FROM").then(|| rest.trim())
    });
    let base = bases.next()?;
    bases.next().is_none().then_some(base)
}

/// The chart lab must build on the node image the hosted lane runs, by the same digest.
fn lab_base_verdict(recipe: &str, node_image: &str) -> Result<(), String> {
    match lab_base(recipe) {
        Some(base) if base == node_image => Ok(()),
        base => Err(format!(
            "{LAB_RECIPE} builds on {}, not the --node-image {node_image}",
            base.unwrap_or("no single FROM image")
        )),
    }
}

/// The chart lab's local tag, keyed by the checkout's canonical path as `quota-fixture` keys its.
fn lab_image(root: &Path) -> String {
    use sha2::Digest as _;
    use std::os::unix::ffi::OsStrExt as _;
    let digest = sha2::Sha256::digest(root.as_os_str().as_bytes());
    format!("{LAB_REPOSITORY}:{}", hex::encode(&digest[..8]))
}

/// This run's own profile name, `substrate-host-exec-v2-lane-<ulid>` with a fresh lowercased ULID
/// (design 33 § 7): a 31-character suffix of `[a-z0-9-]`, within the chart's grammar, and a name
/// no other workload and no other run of the lane holds.
fn lane_profile() -> String {
    format!(
        "{APPARMOR_PROFILE}-lane-{}",
        ulid::Ulid::generate().to_string().to_ascii_lowercase()
    )
}

/// The node-profile `DaemonSet`, by the name the chart gives it. Only its pods carry the
/// `node-profiles` component label; the object itself carries the chart's common labels.
fn node_profiles_daemonset() -> String {
    format!("{RELEASE}-node-profiles")
}

/// What removes the lane's own profile from the kernel, run on the node: the parser reading the
/// exact source the installer wrote for `profile` under the chart's default
/// `nodeProfiles.kubeletRoot`. It names no other profile.
fn removal_argv(profile: &str) -> [String; 3] {
    [
        "/sbin/apparmor_parser".to_owned(),
        "--remove".to_owned(),
        format!("/var/lib/kubelet/seccomp/b10x-substrate/{profile}.apparmor"),
    ]
}

/// What removes `profile` from the host kernel by hand, by its name, without its source.
fn profile_removal(profile: &str) -> String {
    format!("sudo sh -c 'printf %s {profile} > /sys/kernel/security/apparmor/.remove'")
}

/// Whether the shared kernel's `listing` lets this run load `profile`: refused only when that
/// exact name is already loaded, by something this run did not start. Any other profile, the
/// default `substrate-host-exec-v2` and `v1` included, is not the lane's and decides nothing.
fn own_profile_verdict(
    listing: &str,
    profile: &str,
    worker: &str,
) -> std::result::Result<(), String> {
    if profile_loaded(listing, profile) {
        Err(loaded_refusal(worker, profile))
    } else {
        Ok(())
    }
}

/// This run's own name is loaded before the run loaded it: the installer would refuse it on a
/// fresh node, and the lane removes only what its own run loaded, so it leaves it alone.
fn loaded_refusal(worker: &str, profile: &str) -> String {
    format!(
        "this run's own AppArmor profile name {profile} is already loaded in the host kernel kind \
         node {worker} shares, by something this run did not start, and the node-profile \
         installer refuses a loaded profile whose source its fresh node does not hold; this run \
         did not load it, so it neither selects nor removes it"
    )
}

/// One control plane and the pool workers: two hosted, one local, where the worker sees the host's
/// securityfs, without which no `AppArmor` profile can be loaded or applied on it.
fn kind_config(node_profiles: bool) -> String {
    let (worker, count) = if node_profiles {
        (
            "  - role: worker\n    extraMounts:\n      - hostPath: /sys/kernel/security\n        containerPath: /sys/kernel/security\n",
            LOCAL_REPLICAS,
        )
    } else {
        ("  - role: worker\n", REPLICAS)
    };
    format!(
        "kind: Cluster\napiVersion: kind.x-k8s.io/v1alpha4\nnodes:\n  - role: control-plane\n{}",
        worker.repeat(count)
    )
}

/// The lane's values with execution and the node-profile `DaemonSet` enabled, the installer in a
/// `privileged` namespace of its own (the chart refuses the release's namespace), and one daemon
/// pod for the one worker, selecting this run's own `profile` (design 33 § 6).
fn execution_values(values: &Value, profile: &str) -> Value {
    chart::merged(
        values,
        &json!({
            "replicas": LOCAL_REPLICAS,
            "execution": {"enabled": true, "appArmorProfile": profile},
            "nodeProfiles": {"enabled": true, "namespace": NODE_PROFILES_NAMESPACE}
        }),
    )
}

/// The absent scenarios of one run, by name: those a run could not prove, then every
/// execution-enabled scenario.
fn absent_scenarios(outcomes: &[(String, Outcome)]) -> Vec<(String, String)> {
    let mut absent: Vec<(String, String)> = outcomes
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Outcome::Absent(reason) => Some((name.clone(), reason.clone())),
            Outcome::Passed(_) => None,
        })
        .collect();
    absent.extend(
        EXECUTION_ABSENT
            .iter()
            .map(|(name, reason)| ((*name).to_owned(), (*reason).to_owned())),
    );
    absent
}

/// One workflow notice per absent scenario, titled so the release preflight can read them back
/// from the run's annotations. Line breaks and the `%` workflow-command escape are encoded.
pub fn absent_annotations(absent: &[(String, String)]) -> Vec<String> {
    absent
        .iter()
        .map(|(name, reason)| {
            let message = format!("{name}: {reason}")
                .replace('%', "%25")
                .replace('\r', "%0D")
                .replace('\n', "%0A");
            format!("::notice title=chart-lane absent::{message}")
        })
        .collect()
}

/// The names of the absent scenarios, `; `-separated, for the step output and the summary line.
pub fn absent_list(absent: &[(String, String)]) -> String {
    absent
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

struct Lane<'a> {
    args: &'a Args,
    kubeconfig: PathBuf,
    work: PathBuf,
    /// The `AppArmor` profile this run installs and selects: its own lane name in local mode.
    profile: String,
    /// The committed seccomp profile source, read from the recorded commit; local mode only.
    seccomp: Option<String>,
    /// Set by a signal: every command this lane starts afterwards is refused, and a running one is
    /// stopped. Cleanup runs on [`Lane::cleanup`], which ignores it.
    stop: &'a AtomicBool,
}

impl<'a> Lane<'a> {
    fn tool(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env("KUBECONFIG", &self.kubeconfig);
        command
    }

    fn kubectl(&self, args: &[&str]) -> Result<String> {
        output(self.tool(&self.args.kubectl).args(args), self.stop)
    }

    fn helm(&self, args: &[&str]) -> Result<String> {
        output(self.tool(&self.args.helm).args(args), self.stop)
    }

    /// The same lane for cleanup, which a signal does not stop: it removes what the run created.
    fn cleanup(&self) -> Lane<'a> {
        Lane {
            args: self.args,
            kubeconfig: self.kubeconfig.clone(),
            work: self.work.clone(),
            profile: self.profile.clone(),
            seccomp: self.seccomp.clone(),
            stop: &NEVER,
        }
    }
}

/// Set by the first SIGINT or SIGTERM once [`arm_signals`] ran. The lane stops the command it is
/// running, starts no other, and runs its cleanup: the cluster's deletion and, in local mode, the
/// removal of its own profile.
pub(crate) static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The flag cleanup runs under: never set.
static NEVER: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    if INTERRUPTED.swap(true, Ordering::SeqCst) {
        // A second signal leaves at once; the run printed its removal commands when it started.
        // SAFETY: `_exit` is async-signal-safe.
        unsafe { libc::_exit(130) };
    }
}

/// Routes SIGINT and SIGTERM to [`INTERRUPTED`], so a stopped run cleans up before it exits non-zero.
pub fn arm_signals() -> Result<()> {
    let handler: extern "C" fn(libc::c_int) = on_signal;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler touches one lock-free atomic and calls `_exit`, both
        // async-signal-safe.
        let previous = unsafe { libc::signal(signal, handler as libc::sighandler_t) };
        ensure!(
            previous != libc::SIG_ERR,
            "installing the handler for signal {signal} failed"
        );
    }
    Ok(())
}

/// Runs `command` in a process group of its own and returns its standard output. Refused without
/// starting when `stop` is set; stopped, with its whole group, when `stop` is set while it runs.
pub(crate) fn output(command: &mut Command, stop: &AtomicBool) -> Result<String> {
    use std::os::unix::process::CommandExt as _;
    let rendered = format!("{command:?}");
    ensure!(
        !stop.load(Ordering::SeqCst),
        "interrupted by a signal before {rendered} started"
    );
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .with_context(|| format!("starting {rendered}"))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait_until(&mut child, stop, &rendered)?;
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    ensure!(
        status.success(),
        "{rendered} exited {status}: {}{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    Ok(String::from_utf8(stdout)?)
}

/// Reads `pipe` to its end on a thread of its own, so a child never blocks on a full pipe.
fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// The exit status of `child`, a process group leader; `Err` once `stop` is set, after the group
/// is stopped: SIGTERM, then SIGKILL to whatever of it remains after five seconds.
fn wait_until(
    child: &mut Child,
    stop: &AtomicBool,
    rendered: &str,
) -> Result<std::process::ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if stop.load(Ordering::SeqCst) {
            let group = libc::pid_t::try_from(child.id())?;
            // SAFETY: `kill` takes no pointer; a negative pid names the child's own group.
            unsafe { libc::kill(-group, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && !matches!(child.try_wait(), Ok(Some(_))) {
                thread::sleep(Duration::from_millis(50));
            }
            // SAFETY: as above; a group already gone is `ESRCH`, which changes nothing.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            let _ = child.wait();
            bail!("interrupted by a signal: stopped {rendered}");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The command that deletes the lane's cluster by hand.
fn cluster_removal(cluster: &str) -> String {
    format!("kind delete cluster --name {cluster}")
}

/// Deletes the cluster on every exit path. Built only once `kind create` succeeded, so a create
/// refused for a name another run holds never deletes that run's cluster.
struct Cluster<'a> {
    args: &'a Args,
    kubeconfig: PathBuf,
}

impl Drop for Cluster<'_> {
    fn drop(&mut self) {
        use std::os::unix::process::CommandExt as _;
        // Its own process group: a second signal, which ends the lane at once, leaves the
        // deletion running.
        let deleted = Command::new(&self.args.kind)
            .args([
                "delete",
                "cluster",
                "--name",
                &self.args.cluster,
                "--kubeconfig",
            ])
            .arg(&self.kubeconfig)
            .stdin(Stdio::null())
            .process_group(0)
            .status();
        if !deleted
            .as_ref()
            .is_ok_and(std::process::ExitStatus::success)
        {
            eprintln!(
                "chart lane cluster: deleting kind cluster {} failed ({deleted:?}); the next run \
                 refuses until `kind delete cluster --name {}` succeeds",
                self.args.cluster, self.args.cluster
            );
        }
    }
}

/// The lane, stopped by [`INTERRUPTED`]: a run a signal stopped cleans up and exits non-zero,
/// whatever its scenarios came to.
pub fn run(args: &Args) -> Result<ExitCode> {
    let result = run_until(args, &INTERRUPTED);
    if INTERRUPTED.load(Ordering::SeqCst) {
        eprintln!(
            "chart lane cluster: interrupted by a signal; the run removed what it created and is \
             no record"
        );
        return result.and(Ok(ExitCode::FAILURE));
    }
    result
}

#[allow(clippy::too_many_lines)] // The lane in its fixed order: cluster, fixtures, install, six scenarios, teardown.
fn run_until(args: &Args, stop: &AtomicBool) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let (repository, tag) = args
        .image
        .rsplit_once(':')
        .filter(|(repository, _)| !repository.is_empty())
        .ok_or_else(|| anyhow!("--image {:?} is not repository:tag", args.image))?;
    let clusters = output(Command::new(&args.kind).args(["get", "clusters"]), stop)?;
    ensure!(
        !clusters.lines().any(|line| line == args.cluster),
        "a kind cluster named {} already exists; the lane creates and deletes its own. If no \
         other run holds it, remove it with `{}`",
        args.cluster,
        cluster_removal(&args.cluster)
    );

    // Local mode reads `HEAD` and the checkout state once, here; a record names this commit, and
    // the image must carry it.
    let start = if args.node_profiles {
        let sha = git_text(&root, &["rev-parse", "HEAD"])?;
        let clean = git_text(&root, &["status", "--porcelain"])?.is_empty();
        let label = output(
            Command::new(&args.docker).args([
                "image",
                "inspect",
                "--format",
                &format!("{{{{ index .Config.Labels \"{REVISION_LABEL}\" }}}}"),
                &args.image,
            ]),
            stop,
        )?;
        if let Err(reason) = revision_verdict(label.trim(), &sha) {
            eprintln!("{}", refusal(&reason));
            return Ok(ExitCode::FAILURE);
        }
        // The release step counts the committed array, so this binary must declare the same one.
        let committed = git_text(&root, &["show", &format!("{sha}:{SOURCE}")])
            .map(|source| declared_scenarios(&source))
            .unwrap_or_default();
        if committed != Some(SCENARIOS.len()) {
            let committed = committed.map_or_else(
                || "no well-formed SCENARIOS array".to_owned(),
                |count| format!("{count} scenarios"),
            );
            eprintln!(
                "{}",
                refusal(&format!(
                    "{SOURCE} at {sha} declares {committed} and this binary {}; build the lane \
                     from the commit it records",
                    SCENARIOS.len()
                ))
            );
            return Ok(ExitCode::FAILURE);
        }
        // The node image is built from the committed recipe, on the hosted lane's node image.
        let recipe = git_text(&root, &["show", &format!("{sha}:{LAB_RECIPE}")])
            .map_err(|error| format!("{LAB_RECIPE} is not committed at {sha}: {error:#}"))
            .and_then(|recipe| lab_base_verdict(&recipe, &args.node_image).map(|()| recipe));
        let recipe = match recipe {
            Ok(recipe) => recipe,
            Err(reason) => {
                eprintln!("{}", refusal(&reason));
                return Ok(ExitCode::FAILURE);
            }
        };
        Some((sha, clean, recipe))
    } else {
        None
    };

    let work = tempfile::tempdir()?;
    // A record names the commit, so local mode installs the chart and compares the seccomp
    // profile as that commit holds them, like the recipe; the hosted mode records nothing.
    let (chart_dir, seccomp) = match &start {
        Some((sha, _, _)) => match committed_sources(&root, sha, &work.path().join("source")) {
            Ok((chart_dir, seccomp)) => (chart_dir, Some(seccomp)),
            Err(error) => {
                eprintln!("{}", refusal(&format!("{error:#}")));
                return Ok(ExitCode::FAILURE);
            }
        },
        None => (root.join(CHART), None),
    };
    let node_image = match &start {
        Some((_, _, recipe)) => match build_lab(args, &root, recipe, work.path(), stop) {
            Ok(image) => image,
            Err(error) => {
                eprintln!(
                    "{}",
                    refusal(&format!(
                        "building the chart-lab node image failed: {error:#}"
                    ))
                );
                return Ok(ExitCode::FAILURE);
            }
        },
        None => args.node_image.clone(),
    };
    let kubeconfig = work.path().join("kubeconfig");
    let config = work.path().join("kind.yaml");
    fs::write(&config, kind_config(args.node_profiles))?;
    let created = output(
        Command::new(&args.kind)
            .args([
                "create",
                "cluster",
                "--name",
                &args.cluster,
                "--image",
                &node_image,
            ])
            .arg("--config")
            .arg(&config)
            .arg("--kubeconfig")
            .arg(&kubeconfig)
            .args(["--wait", "120s"]),
        stop,
    );
    if let Err(error) = created {
        // A refused create may be refused because another run holds the name: delete nothing.
        // Only a create this run stopped can have left a cluster of its own behind.
        if stop.load(Ordering::SeqCst) {
            eprintln!(
                "chart lane cluster: kind create was stopped; if it left the cluster {}, remove \
                 it with `{}`",
                args.cluster,
                cluster_removal(&args.cluster)
            );
        }
        return Err(error);
    }
    // Armed only now: from here on the cluster is this run's, and every exit path deletes it.
    let cluster = Cluster {
        args,
        kubeconfig: kubeconfig.clone(),
    };
    println!(
        "chart lane cluster: created kind cluster {}; a run killed before it deletes it leaves it \
         behind, removed with `{}`",
        args.cluster,
        cluster_removal(&args.cluster)
    );
    output(
        Command::new(&args.kind).args([
            "load",
            "docker-image",
            &args.image,
            "--name",
            &args.cluster,
        ]),
        stop,
    )?;
    let lane = Lane {
        args,
        kubeconfig,
        work: work.path().to_path_buf(),
        profile: if args.node_profiles {
            lane_profile()
        } else {
            APPARMOR_PROFILE.to_owned()
        },
        seccomp,
        stop,
    };
    if args.node_profiles {
        println!(
            "chart lane cluster: this run's own AppArmor profile is {}; a run killed before it \
             removes it leaves it loaded, removed with `{}`",
            lane.profile,
            profile_removal(&lane.profile)
        );
    }

    let workers = lane.kubectl(&[
        "get",
        "nodes",
        "-l",
        "!node-role.kubernetes.io/control-plane",
        "-o",
        "name",
    ])?;
    let workers: Vec<&str> = workers.lines().collect();
    let wanted = if args.node_profiles {
        LOCAL_REPLICAS
    } else {
        REPLICAS
    };
    ensure!(
        workers.len() == wanted,
        "kind created {} workers, not {wanted}",
        workers.len()
    );
    let names: Vec<&str> = workers
        .iter()
        .map(|worker| worker.strip_prefix("node/").unwrap_or(worker))
        .collect();
    if args.node_profiles
        && let Err(missing) = node_prerequisites(&lane, &names)
    {
        drop(cluster);
        eprintln!("{}", refusal(&missing));
        return Ok(ExitCode::FAILURE);
    }
    for worker in &workers {
        lane.kubectl(&["label", worker, &format!("{POOL_KEY}={POOL_VALUE}")])?;
        // `kubectl taint` takes the type and the name as two arguments, not `node/<name>`.
        let name = worker
            .strip_prefix("node/")
            .with_context(|| format!("kubectl named a worker {worker:?}, not node/<name>"))?;
        lane.kubectl(&[
            "taint",
            "nodes",
            name,
            &format!("{POOL_KEY}={POOL_VALUE}:NoSchedule"),
        ])?;
    }
    let gateway = kind_gateway(args, lane.stop)?;

    let material = Material::new(gateway, &chart_dir, &args.helm)?;
    let identity = Listener::identity(gateway, &material)?;
    let allowed = Listener::counting(gateway)?;
    let denied = Listener::counting(gateway)?;

    // The chart refuses a node-profile installer in the release's own namespace.
    let namespaces: &[(&str, &str)] = if args.node_profiles {
        &[
            (PRIVILEGED_NAMESPACE, "privileged"),
            (NODE_PROFILES_NAMESPACE, "privileged"),
        ]
    } else {
        &[
            (NAMESPACE, "restricted"),
            (PRIVILEGED_NAMESPACE, "privileged"),
            (BASELINE_NAMESPACE, "baseline"),
        ]
    };
    for (namespace, level) in namespaces {
        lane.kubectl(&["create", "namespace", namespace])?;
        lane.kubectl(&[
            "label",
            "namespace",
            namespace,
            &format!("pod-security.kubernetes.io/enforce={level}"),
        ])?;
    }
    let values = cluster_values(repository, tag, gateway, &identity, &allowed, &denied);

    if let Some((sha, clean_before, _)) = start {
        let mut outcomes = Vec::new();
        let mut failures = Vec::new();
        let mut record = |name: &str, result: Result<Outcome>| match result {
            Ok(outcome) => outcomes.push((name.to_owned(), outcome)),
            Err(error) => failures.push(format!("{name}: {error:#}")),
        };
        match execution(&lane, &chart_dir, &material, &values) {
            Ok(results) => {
                for (name, result) in results {
                    record(name, result);
                }
            }
            Err(error) => {
                record(SCENARIOS[0], Err(error.context("did not run")));
                for name in &SCENARIOS[1..] {
                    record(
                        name,
                        Err(anyhow!("did not run: see {} for why", SCENARIOS[0])),
                    );
                }
            }
        }
        // The prerequisites refused this run's own name loaded before the run, so a loaded one
        // is this run's: remove it, and only it, while its node, which holds the source, exists.
        for worker in &names {
            if let Err(error) = remove_own_profile(&lane, worker) {
                let profile = &lane.profile;
                failures.push(format!(
                    "removing this run's {profile} through {worker}: {error:#}; while it is \
                     loaded it stays in the host kernel until the host reboots or root removes it \
                     by name: {}",
                    profile_removal(profile)
                ));
            }
        }
        identity.stop();
        allowed.stop();
        denied.stop();
        drop(cluster);
        return Ok(report_local(
            &root,
            &outcomes,
            &failures,
            &sha,
            clean_before,
        ));
    }

    fixtures(&lane, NAMESPACE, &material)?;
    let values_file = lane.work.join("values.json");
    fs::write(&values_file, serde_json::to_vec_pretty(&values)?)?;
    let values_path = values_file.display().to_string();
    let chart_path = chart_dir.display().to_string();
    lane.helm(&[
        "install",
        RELEASE,
        &chart_path,
        "-n",
        NAMESPACE,
        "--values",
        &values_path,
        "--wait",
        "--timeout",
        "6m",
    ])?;

    let client = material.client()?;
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();
    let mut record = |name: &str, result: Result<Outcome>| match result {
        Ok(outcome) => outcomes.push((name.to_owned(), outcome)),
        Err(error) => failures.push(format!("{name}: {error:#}")),
    };
    record("placement", placement(&lane));
    record(
        "certificate names",
        certificate_names(&lane, &client, &material),
    );
    record("clean-room runner", clean_room_run(&lane, &material));
    record(
        "restart survival",
        restart_survival(&lane, &client, &material, &values),
    );
    record(
        "egress",
        egress(&lane, &client, &material, &allowed, &denied),
    );
    record("pod security", pod_security(&lane, &chart_dir, &values));
    record("uninstall", uninstall(&lane));

    identity.stop();
    allowed.stop();
    denied.stop();
    drop(cluster);

    for (name, outcome) in &outcomes {
        match outcome {
            Outcome::Passed(detail) => println!("chart lane cluster: {name}: passed: {detail}"),
            Outcome::Absent(reason) => {
                println!("chart lane cluster: {name}: absent, not passed: {reason}");
            }
        }
    }
    let passed = outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome, Outcome::Passed(_)))
        .count();
    let absent = absent_scenarios(&outcomes);
    for line in absent_annotations(&absent) {
        println!("{line}");
    }
    if let Some(path) = std::env::var_os("GITHUB_OUTPUT") {
        let mut file = fs::OpenOptions::new().append(true).open(path)?;
        writeln!(file, "absent={}", absent_list(&absent))?;
    }
    if failures.is_empty() {
        println!(
            "chart lane cluster: {passed} of {} scenarios passed, {} absent: {}",
            HOSTED.len() + EXECUTION_ABSENT.len(),
            absent.len(),
            absent_list(&absent)
        );
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("chart lane cluster: {} scenario(s) failed", failures.len());
        for failure in &failures {
            eprintln!("  {failure}");
        }
        Ok(ExitCode::FAILURE)
    }
}

/// What a finished local run prints: standard output, then standard error, and whether it exits 0.
struct Report {
    stdout: Vec<String>,
    stderr: Vec<String>,
    success: bool,
}

/// The lines of one [`Decision`]. A run that passed every scenario and is no record exits 0, as
/// `quota-fixture` does, and its last standard-error line says it is not a release record.
fn report_lines(decision: Decision, sha: &str, at: SystemTime) -> Report {
    match decision {
        Decision::Record(count) => Report {
            stdout: vec![
                format!(
                    "chart lane cluster: all {count} scenarios passed at {sha}; record this line in the annotated tag:"
                ),
                evidence_line(sha, count, at),
            ],
            stderr: Vec::new(),
            success: true,
        },
        Decision::NoRecord(reason) => Report {
            stdout: vec![format!(
                "chart lane cluster: all {} scenarios passed at {sha}, but {reason}; this run is \
                 not a release record for any commit",
                SCENARIOS.len()
            )],
            stderr: vec!["chart lane cluster: not a release record".to_owned()],
            success: true,
        },
        Decision::Failed(reason) => Report {
            stdout: Vec::new(),
            stderr: vec![format!("chart lane cluster: no record: {reason}")],
            success: false,
        },
    }
}

/// The chart and the seccomp profile source as committed at `sha`, extracted with `git archive`
/// into `into`: the directory to install the chart from and the profile's exact text. A local run
/// records `sha`, so it installs what `sha` holds, as it builds the recipe `sha` holds, and never
/// what the working tree holds.
fn committed_sources(root: &Path, sha: &str, into: &Path) -> Result<(PathBuf, String)> {
    fs::create_dir_all(into)?;
    let mut archive = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["archive", "--format=tar", sha, "--", CHART, SECCOMP_SOURCE])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .context("git archive")?;
    let tar = archive.stdout.take().context("piped git archive")?;
    let extracted = Command::new("tar")
        .args(["-x", "-f", "-", "-C"])
        .arg(into)
        .stdin(tar)
        .status()
        .context("tar")?;
    let archived = archive.wait().context("awaiting git archive")?;
    ensure!(
        archived.success() && extracted.success(),
        "extracting {CHART} and {SECCOMP_SOURCE} at {sha} failed: git archive {archived}, tar \
         {extracted}"
    );
    let seccomp = fs::read_to_string(into.join(SECCOMP_SOURCE))
        .with_context(|| format!("{SECCOMP_SOURCE} at {sha}"))?;
    Ok((into.join(CHART), seccomp))
}

/// The local mode's report: every outcome and failure, then the record or why there is none.
fn report_local(
    root: &Path,
    outcomes: &[(String, Outcome)],
    failures: &[String],
    sha: &str,
    clean_before: bool,
) -> ExitCode {
    for (name, outcome) in outcomes {
        match outcome {
            Outcome::Passed(detail) => println!("chart lane cluster: {name}: passed: {detail}"),
            Outcome::Absent(reason) => {
                println!("chart lane cluster: {name}: absent, not passed: {reason}");
            }
        }
    }
    for failure in failures {
        eprintln!("chart lane cluster: failed: {failure}");
    }
    // Read again after a run that takes minutes: a moved `HEAD` or an edited checkout means the
    // record would name a commit nobody is looking at any more. A git failure is no record.
    let after = git_text(root, &["rev-parse", "HEAD"])
        .and_then(|head| Ok((head, git_text(root, &["status", "--porcelain"])?.is_empty())));
    let (head_after, clean_after) = match after {
        Ok(after) => after,
        Err(error) => {
            eprintln!(
                "chart lane cluster: no record: reading the checkout again failed: {error:#}"
            );
            return ExitCode::FAILURE;
        }
    };
    let report = report_lines(
        decision(
            local_verdict(outcomes, failures),
            sha,
            &head_after,
            clean_before,
            clean_after,
        ),
        sha,
        SystemTime::now(),
    );
    for line in &report.stdout {
        println!("{line}");
    }
    for line in &report.stderr {
        eprintln!("{line}");
    }
    if report.success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Whether the kernel's `apparmor/profiles` listing holds `profile` by its exact top-level name,
/// in any mode.
fn profile_loaded(listing: &str, profile: &str) -> bool {
    listing.lines().any(|line| {
        line.strip_prefix(profile)
            .is_some_and(|rest| rest.starts_with(" ("))
    })
}

/// Builds the chart lab's node image from the committed `recipe`, alone in a fresh build context
/// under the lane's work directory, and tags it for this checkout. Never pushed.
fn build_lab(
    args: &Args,
    root: &Path,
    recipe: &str,
    work: &Path,
    stop: &AtomicBool,
) -> Result<String> {
    use std::os::unix::process::CommandExt as _;
    let image = lab_image(&fs::canonicalize(root)?);
    let context = work.join("chart-lab");
    fs::create_dir(&context)?;
    fs::write(context.join("Dockerfile"), recipe)?;
    ensure!(
        !stop.load(Ordering::SeqCst),
        "interrupted by a signal before docker build started"
    );
    let mut child = Command::new(&args.docker)
        .args(["build", "--tag", &image])
        .arg(&context)
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .context("docker build")?;
    let status = wait_until(&mut child, stop, "docker build")?;
    ensure!(
        status.success(),
        "building {image} from {LAB_RECIPE} failed: {status}"
    );
    Ok(image)
}

/// Removes the `AppArmor` profile this run loaded, through the node holding its source, after
/// nothing on the cluster can load it again: the release is uninstalled and no pod of it remains,
/// so the holding installer cannot restart and reload it. A profile already gone is no error; a
/// failed `helm uninstall` is, reported after the profile itself was dealt with. Cleanup: a
/// signal does not stop it.
fn remove_own_profile(lane: &Lane<'_>, worker: &str) -> Result<()> {
    let lane = &lane.cleanup();
    let uninstalled = lane
        .helm(&[
            "uninstall",
            RELEASE,
            "-n",
            PRIVILEGED_NAMESPACE,
            "--wait",
            "--timeout",
            "3m",
        ])
        .context("helm uninstall of the release");
    let removed = remove_loaded_profile(lane, worker);
    match (uninstalled, removed) {
        (Ok(_), removed) => removed,
        (Err(uninstall), Ok(())) => Err(uninstall),
        (Err(uninstall), Err(removal)) => Err(removal.context(format!("{uninstall:#}"))),
    }
}

/// [`remove_own_profile`] after the uninstall: wait until no pod of the release remains, then
/// remove the run's own profile if it is loaded.
fn remove_loaded_profile(lane: &Lane<'_>, worker: &str) -> Result<()> {
    let selector = format!("app.kubernetes.io/instance={RELEASE}");
    let deadline = Instant::now() + Duration::from_mins(3);
    for namespace in [PRIVILEGED_NAMESPACE, NODE_PROFILES_NAMESPACE] {
        loop {
            let pods = lane.kubectl(&[
                "get", "pods", "-n", namespace, "-l", &selector, "-o", "name",
            ])?;
            if pods.trim().is_empty() {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "pods of the release remain in {namespace}: {}",
                pods.trim()
            );
            thread::sleep(Duration::from_secs(2));
        }
    }
    let listing = |lane: &Lane<'_>| {
        on_node(
            lane,
            worker,
            &["cat", "/sys/kernel/security/apparmor/profiles"],
        )
    };
    let profile = &lane.profile;
    if !profile_loaded(&listing(lane)?, profile) {
        return Ok(());
    }
    let argv = removal_argv(profile);
    on_node(lane, worker, &argv.each_ref().map(String::as_str))?;
    ensure!(
        !profile_loaded(&listing(lane)?, profile),
        "{profile} is still loaded after {}",
        argv.join(" ")
    );
    println!("chart lane cluster: removed this run's {profile} from the host kernel");
    Ok(())
}

/// `docker exec <node> <argv>` on one of the lane's own kind nodes.
fn on_node(lane: &Lane<'_>, node: &str, argv: &[&str]) -> Result<String> {
    output(
        Command::new(&lane.args.docker)
            .args(["exec", node])
            .args(argv),
        lane.stop,
    )
}

/// What the node profiles need on each worker, checked before anything is installed: `AppArmor`
/// enabled with its securityfs visible, so the profile can be loaded and applied; the node's own
/// `apparmor_parser`, without which containerd applies no Localhost profile; the lane's profile not
/// yet loaded in the kernel the nodes share, which the installer refuses; and a cgroup v2
/// hierarchy carrying `cpu`, `memory` and `pids`, so the bootstrap can delegate. The first missing
/// prerequisite, named.
fn node_prerequisites(lane: &Lane<'_>, workers: &[&str]) -> std::result::Result<(), String> {
    for worker in workers {
        let enabled = on_node(
            lane,
            worker,
            &["cat", "/sys/module/apparmor/parameters/enabled"],
        );
        if !enabled.as_deref().is_ok_and(|value| value.trim() == "Y") {
            return Err(format!(
                "AppArmor is not enabled in kind node {worker} ({enabled:?}), so the Localhost \
                 AppArmor profile cannot be applied"
            ));
        }
        if on_node(
            lane,
            worker,
            &["test", "-f", "/sys/kernel/security/apparmor/profiles"],
        )
        .is_err()
        {
            return Err(format!(
                "the AppArmor securityfs is not visible in kind node {worker}, so the node profiles \
                 cannot be loaded"
            ));
        }
        // containerd reports AppArmor unsupported, and refuses every Localhost profile with
        // `apparmor is not supported`, on a node without the parser.
        if on_node(lane, worker, &["test", "-x", "/sbin/apparmor_parser"]).is_err() {
            return Err(format!(
                "kind node {worker} has no /sbin/apparmor_parser, so its containerd reports \
                 AppArmor unsupported and refuses the Localhost AppArmor profile"
            ));
        }
        // Every kind node shares the host kernel; the installer refuses a loaded profile whose
        // exact source its own node does not hold. Only this run's own name is the lane's.
        let loaded = on_node(
            lane,
            worker,
            &["cat", "/sys/kernel/security/apparmor/profiles"],
        )
        .map_err(|error| {
            format!("the AppArmor profile listing is not readable in kind node {worker}: {error:#}")
        })?;
        own_profile_verdict(&loaded, &lane.profile, worker)?;
        let filesystem = on_node(lane, worker, &["stat", "-fc", "%T", "/sys/fs/cgroup"]);
        let controllers = on_node(lane, worker, &["cat", "/sys/fs/cgroup/cgroup.controllers"]);
        let delegated = filesystem
            .as_deref()
            .is_ok_and(|value| value.trim() == "cgroup2fs")
            && controllers.as_deref().is_ok_and(|value| {
                ["cpu", "memory", "pids"]
                    .iter()
                    .all(|wanted| value.split_whitespace().any(|have| have == *wanted))
            });
        if !delegated {
            return Err(format!(
                "kind node {worker} has no cgroup v2 hierarchy delegating cpu, memory and pids \
                 ({filesystem:?}, {controllers:?}), so the execution bootstrap cannot delegate"
            ));
        }
    }
    Ok(())
}

/// The daemon's TLS secret and the Identity trust roots in `namespace`.
fn fixtures(lane: &Lane<'_>, namespace: &str, material: &Material) -> Result<()> {
    let certificate = lane.work.join(format!("{namespace}-tls.crt"));
    let key = lane.work.join(format!("{namespace}-tls.key"));
    let identity_ca = lane.work.join(format!("{namespace}-identity-ca.crt"));
    fs::write(&certificate, &material.daemon_pem)?;
    fs::write(&key, &material.daemon_key_pem)?;
    fs::write(&identity_ca, &material.identity_pem)?;
    lane.kubectl(&[
        "create",
        "secret",
        "tls",
        "substrate-tls",
        "-n",
        namespace,
        &format!("--cert={}", certificate.display()),
        &format!("--key={}", key.display()),
    ])?;
    lane.kubectl(&[
        "create",
        "configmap",
        "identity-ca",
        "-n",
        namespace,
        &format!("--from-file=ca.crt={}", identity_ca.display()),
    ])?;
    Ok(())
}

type Results = Vec<(&'static str, Result<Outcome>)>;

/// K9 C: the chart again, with execution and the node profiles, the daemons in the `privileged`
/// namespace and the installer in a `privileged` namespace of its own;
/// the four execution-enabled scenarios against its pods. `Err` when the install never became
/// ready, with what the node-profile `DaemonSet` and the namespace's events say.
fn execution(
    lane: &Lane<'_>,
    chart_dir: &Path,
    material: &Material,
    values: &Value,
) -> Result<Results> {
    let material = material.for_namespace(
        chart_dir,
        &lane.args.helm,
        PRIVILEGED_NAMESPACE,
        LOCAL_REPLICAS,
    )?;
    fixtures(lane, PRIVILEGED_NAMESPACE, &material)?;
    let values = execution_values(values, &lane.profile);
    let values_file = lane.work.join("execution-values.json");
    fs::write(&values_file, serde_json::to_vec_pretty(&values)?)?;
    let installed = lane.helm(&[
        "install",
        RELEASE,
        &chart_dir.display().to_string(),
        "-n",
        PRIVILEGED_NAMESPACE,
        "--values",
        &values_file.display().to_string(),
        "--wait",
        "--timeout",
        "8m",
    ]);
    if let Err(error) = installed {
        return Err(error.context(installation_diagnosis(lane)));
    }
    let client = material.client()?;
    let pods = (0..LOCAL_REPLICAS)
        .map(|ordinal| DaemonProcess::observe(lane, ordinal))
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![
        (
            "execution bootstrap",
            execution_bootstrap(lane, &client, &material, &pods),
        ),
        ("Localhost seccomp profile", localhost_seccomp(lane, &pods)),
        (
            "Localhost AppArmor profile",
            localhost_apparmor(lane, &pods),
        ),
        (
            "TLS key after the uid drop",
            key_after_drop(lane, &client, &material, &pods),
        ),
    ])
}

/// Why the execution-enabled install did not become ready, as far as the cluster says.
fn installation_diagnosis(lane: &Lane<'_>) -> String {
    let ask = |args: &[&str]| {
        lane.kubectl(args)
            .unwrap_or_else(|error| format!("<{error:#}>"))
    };
    let profiles = ask(&[
        "get",
        "daemonset",
        "-n",
        NODE_PROFILES_NAMESPACE,
        &node_profiles_daemonset(),
        "-o",
        "jsonpath={.metadata.name} ready {.status.numberReady} of {.status.desiredNumberScheduled}",
    ]);
    let logs = ask(&[
        "logs",
        "-n",
        NODE_PROFILES_NAMESPACE,
        "-l",
        "app.kubernetes.io/component=node-profiles",
        "--tail",
        "20",
    ]);
    let daemon_logs = ask(&[
        "logs",
        "-n",
        PRIVILEGED_NAMESPACE,
        "-l",
        SELECTOR,
        "-c",
        "daemon",
        "--tail",
        "20",
    ]);
    let events = ask(&[
        "get",
        "events",
        "-n",
        PRIVILEGED_NAMESPACE,
        "--field-selector",
        "type=Warning",
        "-o",
        "jsonpath={range .items[*]}{.involvedObject.name}: {.reason}: {.message}{\"\\n\"}{end}",
    ]);
    format!(
        "the execution-enabled install never became ready. Node profiles: {profiles}. Node-profile \
         logs: {logs}. Daemon logs: {daemon_logs}. Warning events: {events}"
    )
}

/// One execution-enabled daemon pod, observed from its kind node: the container's init (PID 1,
/// which the bootstrap became) and its one child, the daemon.
struct DaemonProcess {
    ordinal: usize,
    node: String,
    container: Value,
    /// The daemon container's containerd id, for `crictl exec` diagnostics on its node.
    container_id: String,
    runtime: Value,
    init: String,
    init_status: String,
    daemon: String,
    daemon_status: String,
    daemon_argv: Vec<String>,
}

impl DaemonProcess {
    fn observe(lane: &Lane<'_>, ordinal: usize) -> Result<Self> {
        let pod: Value = serde_json::from_str(&lane.kubectl(&[
            "get",
            "pod",
            "-n",
            PRIVILEGED_NAMESPACE,
            &format!("{RELEASE}-{ordinal}"),
            "-o",
            "json",
        ])?)?;
        let node = pod["spec"]["nodeName"]
            .as_str()
            .ok_or_else(|| anyhow!("{RELEASE}-{ordinal} is on no node"))?
            .to_owned();
        let container = pod["spec"]["containers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|container| container["name"] == "daemon")
            .cloned()
            .ok_or_else(|| anyhow!("{RELEASE}-{ordinal} has no daemon container"))?;
        let id = pod["status"]["containerStatuses"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|status| status["name"] == "daemon")
            .and_then(|status| status["containerID"].as_str())
            .and_then(|id| id.strip_prefix("containerd://"))
            .ok_or_else(|| anyhow!("{RELEASE}-{ordinal} reports no containerd daemon container"))?
            .to_owned();
        let inspected: Value =
            serde_json::from_str(&on_node(lane, &node, &["crictl", "inspect", &id])?)?;
        let init = inspected["info"]["pid"]
            .as_u64()
            .ok_or_else(|| anyhow!("crictl reports no pid for {id}"))?
            .to_string();
        let children = on_node(
            lane,
            &node,
            &["cat", &format!("/proc/{init}/task/{init}/children")],
        )?;
        let children: Vec<&str> = children.split_whitespace().collect();
        let [daemon] = children[..] else {
            bail!("the init of {RELEASE}-{ordinal} has children {children:?}, not one daemon");
        };
        let daemon = daemon.to_owned();
        let read = |path: String| on_node(lane, &node, &["cat", &path]);
        let init_status = read(format!("/proc/{init}/status"))?;
        let daemon_status = read(format!("/proc/{daemon}/status"))?;
        let daemon_argv = read(format!("/proc/{daemon}/cmdline"))?
            .split('\0')
            .filter(|argument| !argument.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Self {
            ordinal,
            runtime: inspected["info"]["runtimeSpec"].clone(),
            container_id: id,
            node,
            container,
            init,
            init_status,
            daemon,
            daemon_status,
            daemon_argv,
        })
    }

    fn name(&self) -> String {
        format!("{RELEASE}-{}", self.ordinal)
    }

    /// The `AppArmor` label of `pid`, from the node.
    fn label(&self, lane: &Lane<'_>, pid: &str) -> Result<String> {
        on_node(
            lane,
            &self.node,
            &["cat", &format!("/proc/{pid}/attr/apparmor/current")],
        )
        .or_else(|_| {
            on_node(
                lane,
                &self.node,
                &["cat", &format!("/proc/{pid}/attr/current")],
            )
        })
        .map(|label| label.trim_end_matches(['\0', '\n']).to_owned())
    }
}

/// The value of one `/proc/<pid>/status` field.
fn status_field<'a>(status: &'a str, field: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        line.strip_prefix(field)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(str::trim)
    })
}

/// Every uid and gid is 65532 and no capability is effective, permitted or in the bounding set.
fn dropped(status: &str) -> std::result::Result<(), String> {
    for field in ["Uid", "Gid"] {
        let ids: Vec<&str> = status_field(status, field)
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        if ids.len() != 4 || ids.iter().any(|id| *id != DAEMON_UID) {
            return Err(format!("{field} is {ids:?}"));
        }
    }
    for field in ["CapEff", "CapPrm", "CapBnd", "CapInh", "CapAmb"] {
        let value = status_field(status, field).unwrap_or_default();
        if u64::from_str_radix(value, 16) != Ok(0) {
            return Err(format!("{field} is {value:?}"));
        }
    }
    Ok(())
}

fn execution_bootstrap(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
    pods: &[DaemonProcess],
) -> Result<Outcome> {
    for pod in pods {
        dropped(&pod.init_status)
            .map_err(|reason| anyhow!("{} init {}: {reason}", pod.name(), pod.init))?;
        dropped(&pod.daemon_status)
            .map_err(|reason| anyhow!("{} daemon {}: {reason}", pod.name(), pod.daemon))?;
        ensure!(
            status_field(&pod.daemon_status, "NoNewPrivs") == Some("1"),
            "{} daemon runs without no_new_privs",
            pod.name()
        );
        let argv = &pod.daemon_argv;
        ensure!(
            argv.first().map(String::as_str) == Some("/usr/local/bin/substrate-daemon")
                && argv
                    .windows(2)
                    .any(|pair| pair[0] == "--cgroup-root" && pair[1] == "/sys/fs/cgroup"),
            "{} runs {argv:?}, not the daemon on the delegated cgroup root",
            pod.name()
        );
    }

    let forward = Forward::open_in(lane, PRIVILEGED_NAMESPACE, 0)?;
    let pod = Target::pod(&forward, client, material, 0);
    let (status, machine) = call(&pod, "GET", "/v1/machine", None)?;
    ensure!(status == 200, "machine answered {status}: {machine}");
    let snapshot = machine["result"]["snapshot"]
        .as_str()
        .ok_or_else(|| anyhow!("the machine names no capability snapshot: {machine}"))?
        .to_owned();
    let create = json!({"op": ulid::Ulid::generate().to_string(), "input": {"source": "empty", "labels": {"lane": "execution"}}});
    let (status, created) = call(&pod, "POST", "/v1/workspaces", Some(&create))?;
    ensure!(
        status == 201,
        "workspace create answered {status}: {created}"
    );
    let workspace = created["result"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let exec = json!({"op": ulid::Ulid::generate().to_string(), "input": {
        "workspace": workspace,
        "argv": ["/usr/bin/printf", "hello"],
        "env": {"allow": [], "set": {}},
        "sandbox": {"capability_snapshot": snapshot, "network": "none", "profile": "workspace", "require": true},
        "limits": {"timeout_ms": 20_000, "output_bytes": 65_536, "processes": 16, "memory_bytes": 67_108_864, "cpu_millis": 1_000},
        "wait": true
    }});
    let (status, executed) = call(&pod, "POST", "/v1/execs", Some(&exec))?;
    if !(status == 200
        && executed["result"]["state"] == "exited"
        && executed["result"]["exit"] == json!({"code": 0, "signal": null})
        && executed["result"]["applied"]["network"] == "none")
    {
        bail!(
            "the confined exec answered {status}: {executed}; {}",
            exec_diagnosis(lane, &pods[0], &machine)
        );
    }
    let id = executed["result"]["id"].as_str().unwrap_or_default();
    let (status, stdout) = call(
        &pod,
        "GET",
        &format!("/v1/execs/{id}/output?stream=stdout&offset=0&limit_bytes=65536"),
        None,
    )?;
    ensure!(
        status == 200 && stdout["result"]["content"]["data"] == "aGVsbG8=",
        "the confined exec's stdout is not hello: {status} {stdout}"
    );
    Ok(Outcome::Passed(format!(
        "init and daemon of {} run as uid {DAEMON_UID} with no capability and no_new_privs on the \
         delegated cgroup root; {id} ran /usr/bin/printf confined in {workspace} and printed hello",
        pods.iter()
            .map(DaemonProcess::name)
            .collect::<Vec<_>>()
            .join(" and ")
    )))
}

/// What the daemon's confinement probe could have tripped on, as its node and its own container
/// see it: the capability facts it published, its cgroup, the delegated root it writes, and one
/// bubblewrap sandbox started inside its container under the same `AppArmor` and seccomp profiles.
fn exec_diagnosis(lane: &Lane<'_>, pod: &DaemonProcess, machine: &Value) -> String {
    let answer = |result: Result<String>| match result {
        Ok(text) => text.trim().to_owned(),
        Err(error) => format!("<{error:#}>"),
    };
    let inside = |argv: &[&str]| {
        let mut command = vec!["crictl", "exec", pod.container_id.as_str()];
        command.extend_from_slice(argv);
        answer(on_node(lane, &pod.node, &command))
    };
    let facts = machine["result"]["facts"].to_string();
    let cgroup = answer(on_node(
        lane,
        &pod.node,
        &["cat", &format!("/proc/{}/cgroup", pod.daemon)],
    ));
    let root = inside(&[
        "ls",
        "-ln",
        "/sys/fs/cgroup/cgroup.procs",
        "/sys/fs/cgroup/cgroup.subtree_control",
    ]);
    let controllers = inside(&["cat", "/sys/fs/cgroup/cgroup.controllers"]);
    let sandbox = inside(&DIAGNOSIS_SANDBOX);
    let denials = answer(on_node(lane, &pod.node, &["dmesg"]).map(|log| {
        let lines = apparmor_denials(&log, &lane.profile);
        lines[lines.len().saturating_sub(8)..].join("\n")
    }));
    format!(
        "diagnosis of {}: published facts {facts}; daemon cgroup {cgroup:?}; delegated root \
         {root:?} with controllers {controllers:?}; a bubblewrap sandbox in its container as the \
         daemon runs it: {sandbox:?}; kernel AppArmor denials under {}: {denials:?}",
        pod.name(),
        lane.profile
    )
}

/// The confinement probe's sandbox, started the way the daemon starts it: uid and gid 65532, no
/// capability, `no_new_privs`, and a read-only bind of a fresh directory under `/tmp`, where the
/// probe's sentinel lives. Run as the container's root with its capabilities instead, bubblewrap
/// fails on steps the daemon never takes.
const DIAGNOSIS_SANDBOX: [&str; 11] = [
    "setpriv",
    "--reuid=65532",
    "--regid=65532",
    "--clear-groups",
    "--inh-caps=-all",
    "--bounding-set=-all",
    "--no-new-privs",
    "--",
    "/bin/sh",
    "-c",
    "d=$(mktemp -d) && /usr/bin/bwrap --unshare-user --disable-userns --unshare-ipc \
     --unshare-pid --unshare-net --unshare-uts --new-session --die-with-parent --clearenv \
     --ro-bind /usr /usr --ro-bind-try /bin /bin --ro-bind-try /lib /lib --ro-bind-try /lib64 \
     /lib64 --proc /proc --dev /dev --tmpfs /tmp --assert-userns-disabled --dir /runtime \
     --ro-bind \"$d\" /runtime/sentinel -- /usr/bin/true; s=$?; rmdir \"$d\"; exit $s",
];

/// The kernel's `AppArmor` denials of exactly `profile`, in log order.
fn apparmor_denials<'a>(log: &'a str, profile: &str) -> Vec<&'a str> {
    let label = format!("profile=\"{profile}\"");
    log.lines()
        .filter(|line| line.contains("apparmor=\"DENIED\"") && line.contains(&label))
        .collect()
}

/// The syscall names of a seccomp document, profile or OCI runtime spec alike.
fn syscall_names(seccomp: &Value) -> BTreeSet<String> {
    seccomp["syscalls"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|rule| rule["names"].as_array().into_iter().flatten())
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
}

fn localhost_seccomp(lane: &Lane<'_>, pods: &[DaemonProcess]) -> Result<Outcome> {
    let source = lane
        .seccomp
        .as_deref()
        .with_context(|| format!("no committed {SECCOMP_SOURCE} was read for this run"))?;
    let profile: Value = serde_json::from_str(source)?;
    let names = syscall_names(&profile);
    ensure!(!names.is_empty(), "{SECCOMP_SOURCE} names no syscall");
    for pod in pods {
        let declared = &pod.container["securityContext"]["seccompProfile"];
        ensure!(
            *declared == json!({"type": "Localhost", "localhostProfile": SECCOMP_PROFILE}),
            "{} declares seccomp {declared}",
            pod.name()
        );
        let installed = on_node(
            lane,
            &pod.node,
            &[
                "cat",
                &format!("/var/lib/kubelet/seccomp/{SECCOMP_PROFILE}"),
            ],
        )?;
        ensure!(
            installed == source,
            "the profile on {} is not {SECCOMP_SOURCE} byte for byte",
            pod.node
        );
        let applied = &pod.runtime["linux"]["seccomp"];
        ensure!(
            applied["defaultAction"] == profile["defaultAction"] && syscall_names(applied) == names,
            "{} runs seccomp with default {} over {} syscall names, not the profile's {} over {}",
            pod.name(),
            applied["defaultAction"],
            syscall_names(applied).len(),
            profile["defaultAction"],
            names.len()
        );
        for (role, status) in [("init", &pod.init_status), ("daemon", &pod.daemon_status)] {
            ensure!(
                status_field(status, "Seccomp") == Some("2"),
                "{} {role} is not in seccomp filter mode",
                pod.name()
            );
        }
    }
    Ok(Outcome::Passed(format!(
        "init and daemon of each pod are in seccomp filter mode under {SECCOMP_PROFILE}, which its \
         node holds byte for byte: default {} over {} syscall names",
        profile["defaultAction"],
        names.len()
    )))
}

fn localhost_apparmor(lane: &Lane<'_>, pods: &[DaemonProcess]) -> Result<Outcome> {
    let ready = lane.kubectl(&[
        "get",
        "daemonset",
        "-n",
        NODE_PROFILES_NAMESPACE,
        &node_profiles_daemonset(),
        "-o",
        "jsonpath={.status.numberReady}",
    ])?;
    ensure!(
        ready.trim() == pods.len().to_string(),
        "{ready} node-profile pods are ready, not {}",
        pods.len()
    );
    let profile = lane.profile.as_str();
    let enforced = format!("{profile} (enforce)");
    for pod in pods {
        let declared = &pod.container["securityContext"]["appArmorProfile"];
        ensure!(
            *declared == json!({"type": "Localhost", "localhostProfile": profile}),
            "{} declares AppArmor {declared}",
            pod.name()
        );
        ensure!(
            pod.runtime["process"]["apparmorProfile"] == profile,
            "{} runs under AppArmor {}",
            pod.name(),
            pod.runtime["process"]["apparmorProfile"]
        );
        for (role, pid) in [("init", &pod.init), ("daemon", &pod.daemon)] {
            let label = pod.label(lane, pid)?;
            ensure!(
                label == enforced,
                "{} {role} is labelled {label:?}, not {enforced:?}",
                pod.name()
            );
        }
    }
    Ok(Outcome::Passed(format!(
        "the node profiles are ready on {} worker(s); init and daemon of each pod are labelled \
         {enforced}",
        pods.len()
    )))
}

fn key_after_drop(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
    pods: &[DaemonProcess],
) -> Result<Outcome> {
    for pod in pods {
        dropped(&pod.daemon_status).map_err(|reason| anyhow!("{} daemon: {reason}", pod.name()))?;
        let key = on_node(
            lane,
            &pod.node,
            &[
                "stat",
                "-c",
                "%a %u %g %F",
                &format!("/proc/{}/root/run/substrate/tls/tls.key", pod.daemon),
            ],
        )?;
        ensure!(
            key.trim() == format!("400 {DAEMON_UID} {DAEMON_UID} regular file"),
            "{}'s key is {key:?}",
            pod.name()
        );
        let forward = Forward::open_in(lane, PRIVILEGED_NAMESPACE, pod.ordinal)?;
        let target = Target::pod(&forward, client, material, pod.ordinal);
        let (status, machine) = call(&target, "GET", "/v1/machine", None)?;
        ensure!(
            status == 200,
            "{} answered {status} over TLS: {machine}",
            pod.name()
        );
    }
    Ok(Outcome::Passed(format!(
        "each key is 0400 and owned by {DAEMON_UID}; each daemon, uid {DAEMON_UID} with no \
         capability, served TLS verified under its own pod name"
    )))
}

fn kind_gateway(args: &Args, stop: &AtomicBool) -> Result<Ipv4Addr> {
    let config = output(
        Command::new(&args.docker).args([
            "network",
            "inspect",
            "kind",
            "--format",
            "{{json .IPAM.Config}}",
        ]),
        stop,
    )?;
    let config: Value = serde_json::from_str(config.trim())?;
    config
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| entry["Gateway"].as_str()?.parse::<Ipv4Addr>().ok())
        .ok_or_else(|| anyhow!("the kind network has no IPv4 gateway: {config}"))
}

/// The lane's certificates and the one bearer credential its Identity stub resolves. The
/// namespace lane's step 4 makes its own with [`Material::generate`].
pub(crate) struct Material {
    pub(crate) daemon_pem: String,
    pub(crate) daemon_key_pem: String,
    daemon_der: CertificateDer<'static>,
    pub(crate) identity_pem: String,
    identity_der: CertificateDer<'static>,
    identity_key: Vec<u8>,
    pod_names: Vec<String>,
    service_name: String,
    pub(crate) credential: String,
}

impl Material {
    /// The per-pod host names the chart's `certificate-names` `ConfigMap` computes in `namespace`
    /// for `replicas` pods.
    fn pod_names(
        chart_dir: &Path,
        helm: &Path,
        namespace: &str,
        replicas: usize,
    ) -> Result<Vec<String>> {
        let values = chart::merged(&chart::base_values(), &json!({"replicas": replicas}));
        let rendered = chart::render(helm, chart_dir, RELEASE, namespace, &values)?
            .map_err(|stderr| anyhow!("helm template: {stderr}"))?;
        let names = chart_yaml::documents(&rendered)?
            .into_iter()
            .find(|document| {
                document["kind"] == "ConfigMap"
                    && document["metadata"]["name"] == format!("{RELEASE}-certificate-names")
            })
            .and_then(|document| document["data"]["dns-names"].as_str().map(str::to_owned))
            .ok_or_else(|| anyhow!("the chart rendered no certificate-names ConfigMap"))?;
        let pod_names: Vec<String> = names.lines().map(str::to_owned).collect();
        ensure!(pod_names.len() == replicas, "the chart names {pod_names:?}");
        Ok(pod_names)
    }

    /// The same Identity stub and credential, with a daemon certificate naming exactly the
    /// `replicas` pods of the release in `namespace`.
    fn for_namespace(
        &self,
        chart_dir: &Path,
        helm: &Path,
        namespace: &str,
        replicas: usize,
    ) -> Result<Self> {
        let pod_names = Self::pod_names(chart_dir, helm, namespace, replicas)?;
        let daemon_key = KeyPair::generate()?;
        let daemon = CertificateParams::new(pod_names.clone())?.self_signed(&daemon_key)?;
        Ok(Self {
            daemon_pem: daemon.pem(),
            daemon_key_pem: daemon_key.serialize_pem(),
            daemon_der: daemon.der().clone(),
            identity_pem: self.identity_pem.clone(),
            identity_der: self.identity_der.clone(),
            identity_key: self.identity_key.clone(),
            pod_names,
            service_name: format!("{RELEASE}.{namespace}.svc.cluster.local"),
            credential: self.credential.clone(),
        })
    }

    fn new(gateway: Ipv4Addr, chart_dir: &Path, helm: &Path) -> Result<Self> {
        let pod_names = Self::pod_names(chart_dir, helm, NAMESPACE, REPLICAS)?;
        Self::generate(
            pod_names,
            format!("{RELEASE}.{NAMESPACE}.svc.cluster.local"),
            gateway,
        )
    }

    /// A daemon certificate naming exactly `pod_names`, one `service_name` it must not carry, an
    /// Identity stub certificate for `identity_host` and a fresh credential the stub resolves.
    pub(crate) fn generate(
        pod_names: Vec<String>,
        service_name: String,
        identity_host: Ipv4Addr,
    ) -> Result<Self> {
        let daemon_key = KeyPair::generate()?;
        let daemon = CertificateParams::new(pod_names.clone())?.self_signed(&daemon_key)?;
        let identity_key = KeyPair::generate()?;
        let identity =
            CertificateParams::new(vec![identity_host.to_string()])?.self_signed(&identity_key)?;
        let secret = ulid::Ulid::generate()
            .to_string()
            .repeat(2)
            .to_ascii_lowercase();
        let credential = format!("identity_access_v1_{}", &secret[..43]);
        Ok(Self {
            daemon_pem: daemon.pem(),
            daemon_key_pem: daemon_key.serialize_pem(),
            daemon_der: daemon.der().clone(),
            identity_pem: identity.pem(),
            identity_der: identity.der().clone(),
            identity_key: identity_key.serialize_der(),
            pod_names,
            service_name,
            credential,
        })
    }

    pub(crate) fn client(&self) -> Result<Arc<ClientConfig>> {
        let mut roots = RootCertStore::empty();
        roots.add(self.daemon_der.clone())?;
        Ok(Arc::new(
            ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
        ))
    }
}

/// A listener on the kind gateway (or loopback, for the namespace lane), served from a thread
/// until stopped.
pub(crate) struct Listener {
    address: SocketAddr,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Listener {
    fn serve(
        gateway: Ipv4Addr,
        handle: impl Fn(TcpStream) + Send + Sync + 'static,
    ) -> Result<Self> {
        let listener = TcpListener::bind((gateway, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = Arc::new(handle);
        let thread = {
            let accepted = Arc::clone(&accepted);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            accepted.fetch_add(1, Ordering::SeqCst);
                            let handle = Arc::clone(&handle);
                            thread::spawn(move || {
                                let _ = stream.set_nonblocking(false);
                                handle(stream);
                            });
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => return,
                    }
                }
            })
        };
        Ok(Self {
            address,
            accepted,
            stop,
            thread: Some(thread),
        })
    }

    fn counting(gateway: Ipv4Addr) -> Result<Self> {
        Self::serve(gateway, drop)
    }

    pub(crate) fn identity(gateway: Ipv4Addr, material: &Material) -> Result<Self> {
        let builder = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth();
        // The refusal is not propagated: nothing that came out of the call holding the key may
        // reach a printed line. The pair is generated a few lines earlier, so a refusal is a bug.
        let Ok(config) = builder.with_single_cert(
            vec![material.identity_der.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(material.identity_key.clone())),
        ) else {
            bail!("rustls refused the Identity stub's own generated certificate and key");
        };
        let config = Arc::new(config);
        let credential = material.credential.clone();
        let origin = Arc::new(std::sync::OnceLock::<String>::new());
        let served_origin = Arc::clone(&origin);
        let listener = Self::serve(gateway, move |stream| {
            let origin = served_origin.get().cloned().unwrap_or_default();
            let _ = resolve(stream, &config, &credential, &origin);
        })?;
        origin
            .set(format!("https://{}", listener.address))
            .map_err(|_| anyhow!("the Identity origin was set twice"))?;
        Ok(listener)
    }

    pub(crate) fn origin(&self) -> String {
        format!("https://{}", self.address)
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One `GET /v1/access-authority` answer, then close.
fn resolve(
    stream: TcpStream,
    config: &Arc<ServerConfig>,
    credential: &str,
    origin: &str,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut tls = StreamOwned::new(ServerConnection::new(Arc::clone(config))?, stream);
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = tls.read(&mut buffer)?;
        ensure!(count > 0 && request.len() < 16 * 1024, "incomplete request");
        request.extend_from_slice(&buffer[..count]);
    }
    let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
    let lines: Vec<&str> = head.lines().collect();
    let header = |name: &str| {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{name}:")).map(str::trim))
    };
    let raw = String::from_utf8_lossy(&request).into_owned();
    let bearer = raw
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_owned())
        })
        .and_then(|value| value.strip_prefix("Bearer ").map(str::to_owned));
    let (status, body) = if !lines
        .first()
        .is_some_and(|line| line.starts_with("get /v1/access-authority "))
        || header("x-b10x-audience") != Some(AUDIENCE)
    {
        ("400 Bad Request", String::new())
    } else if bearer.as_deref() != Some(credential) {
        ("401 Unauthorized", String::new())
    } else {
        let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        let authority = json!({
            "iss": origin,
            "sub": "chart-lane-subject",
            "aud": AUDIENCE,
            "iat": now,
            "nbf": now,
            "exp": now + 300,
            "jti": ulid::Ulid::generate().to_string(),
            "act": {"sub": "chart-lane-actor"},
            "scope": "exec observe workspaces",
            "principal_kind": "human",
            "tenant_id": "chart-lane-tenant",
            "email": null,
            "groups": []
        });
        ("200 OK", authority.to_string())
    };
    write!(
        tls,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    tls.conn.send_close_notify();
    tls.flush()?;
    Ok(())
}

fn cluster_values(
    repository: &str,
    tag: &str,
    gateway: Ipv4Addr,
    identity: &Listener,
    allowed: &Listener,
    denied: &Listener,
) -> Value {
    let host = format!("{gateway}/32");
    chart::merged(
        &chart::base_values(),
        &json!({
            "image": {"repository": repository, "tag": tag, "digest": "", "pullPolicy": "Never"},
            "nodePool": {
                "label": {"key": POOL_KEY, "value": POOL_VALUE},
                "taint": {"key": POOL_KEY, "value": POOL_VALUE, "effect": "NoSchedule"}
            },
            "persistence": {"storageClassName": "standard", "size": "1Gi"},
            "identity": {
                "origin": identity.origin(),
                "caConfigMap": {"name": "identity-ca"},
                "egress": {"cidr": host, "port": identity.address.port()}
            },
            "networkPolicy": {"clients": [], "scrapers": []},
            "resources": {
                "requests": {"cpu": "100m", "memory": "128Mi"},
                "limits": {"cpu": "1", "memory": "1Gi"}
            },
            "daemon": {
                "gitSources": [
                    {"name": "declared", "urlPrefix": format!("https://{}/", allowed.address), "cidr": host, "port": allowed.address.port()},
                    {"name": "undeclared", "urlPrefix": format!("https://{}/", denied.address), "cidr": "192.0.2.20/32", "port": denied.address.port()}
                ],
                "egressApertures": []
            }
        }),
    )
}

/// `kubectl port-forward` to one pod's control port, stopped on drop.
struct Forward(Child, u16);

impl Forward {
    fn open(lane: &Lane<'_>, ordinal: usize) -> Result<Self> {
        Self::open_in(lane, NAMESPACE, ordinal)
    }

    fn open_in(lane: &Lane<'_>, namespace: &str, ordinal: usize) -> Result<Self> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let child = lane
            .tool(&lane.args.kubectl)
            .args([
                "port-forward",
                "-n",
                namespace,
                &format!("pod/{RELEASE}-{ordinal}"),
                &format!("{port}:8443"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let forward = Self(child, port);
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            ensure!(
                Instant::now() < deadline,
                "port-forward to {RELEASE}-{ordinal} never listened"
            );
            thread::sleep(Duration::from_millis(200));
        }
        Ok(forward)
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Where one request goes: a forwarded pod port, verified for `name`.
struct Target<'a> {
    forward: &'a Forward,
    client: &'a Arc<ClientConfig>,
    name: &'a str,
    credential: &'a str,
    timeout: Duration,
    /// The transient Git source authorization a Git workspace create carries; without it the
    /// daemon refuses `workspace.git-authority-absent` before it dials anything.
    source_authorization: Option<&'a str>,
}

impl<'a> Target<'a> {
    fn pod(
        forward: &'a Forward,
        client: &'a Arc<ClientConfig>,
        material: &'a Material,
        ordinal: usize,
    ) -> Self {
        Self {
            forward,
            client,
            name: &material.pod_names[ordinal],
            credential: &material.credential,
            timeout: Duration::from_secs(30),
            source_authorization: None,
        }
    }
}

/// One request over TLS verified for the target's name; the TLS error itself when that fails.
fn https(
    target: &Target<'_>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<std::result::Result<(u16, Value), rustls::Error>> {
    let Target {
        forward,
        client,
        name,
        credential,
        timeout,
        source_authorization,
    } = *target;
    let stream = TcpStream::connect(("127.0.0.1", forward.1))?;
    stream.set_read_timeout(Some(timeout))?;
    let server = ServerName::try_from(name.to_owned())?;
    let mut tls = StreamOwned::new(ClientConnection::new(Arc::clone(client), server)?, stream);
    let body = body.map(Value::to_string).unwrap_or_default();
    let authorization = source_authorization
        .map(|value| format!("x-b10x-workspace-source-authorization: {value}\r\n"))
        .unwrap_or_default();
    let content = if body.is_empty() {
        String::new()
    } else {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    };
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {name}\r\nAuthorization: Bearer {credential}\r\n{authorization}{content}Connection: close\r\n\r\n{body}"
    );
    let mut response = Vec::new();
    let result = tls
        .write_all(request.as_bytes())
        .and_then(|()| tls.read_to_end(&mut response));
    if let Err(error) = result {
        if let Some(tls_error) = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        {
            return Ok(Err(tls_error.clone()));
        }
        if error.kind() != ErrorKind::UnexpectedEof || response.is_empty() {
            return Err(error.into());
        }
    }
    let text = String::from_utf8_lossy(&response);
    let (head, payload) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("incomplete response: {text}"))?;
    let status: u16 = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| anyhow!("no status in {head}"))?;
    let payload = if payload.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(payload.trim()).unwrap_or_else(|_| Value::String(payload.to_owned()))
    };
    Ok(Ok((status, payload)))
}

fn call(
    target: &Target<'_>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value)> {
    https(target, method, path, body)?.map_err(|error| anyhow!("TLS to {}: {error}", target.name))
}

fn daemon_pods(lane: &Lane<'_>) -> Result<Vec<Value>> {
    let pods: Value = serde_json::from_str(
        &lane.kubectl(&["get", "pods", "-n", NAMESPACE, "-l", SELECTOR, "-o", "json"])?,
    )?;
    Ok(pods["items"].as_array().cloned().unwrap_or_default())
}

fn placement(lane: &Lane<'_>) -> Result<Outcome> {
    let pods = daemon_pods(lane)?;
    ensure!(
        pods.len() == REPLICAS,
        "{} daemon pods, not {REPLICAS}",
        pods.len()
    );
    let mut nodes = BTreeSet::new();
    for pod in &pods {
        let ready = pod["status"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|condition| condition["type"] == "Ready" && condition["status"] == "True");
        ensure!(ready, "{} is not ready", pod["metadata"]["name"]);
        let node = pod["spec"]["nodeName"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let labels: Value = serde_json::from_str(&lane.kubectl(&[
            "get",
            "node",
            &node,
            "-o",
            "jsonpath={.metadata.labels}",
        ])?)?;
        ensure!(labels[POOL_KEY] == POOL_VALUE, "{node} is outside the pool");
        nodes.insert(node);
    }
    ensure!(
        nodes.len() == REPLICAS,
        "two daemons share a node: {nodes:?}"
    );
    Ok(Outcome::Passed(format!(
        "{REPLICAS} ready pods on {nodes:?}"
    )))
}

fn certificate_names(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
) -> Result<Outcome> {
    for ordinal in 0..REPLICAS {
        let forward = Forward::open(lane, ordinal)?;
        let pod = Target::pod(&forward, client, material, ordinal);
        let (status, payload) = call(&pod, "GET", "/v1/machine", None)?;
        ensure!(
            status == 200,
            "{} answered {status}: {payload}",
            material.pod_names[ordinal]
        );
        let service = Target {
            name: &material.service_name,
            timeout: Duration::from_secs(10),
            ..pod
        };
        match https(&service, "GET", "/v1/machine", None)? {
            Err(rustls::Error::InvalidCertificate(_)) => {}
            other => bail!(
                "{} verified under the Service name {}: {other:?}",
                material.pod_names[ordinal],
                material.service_name
            ),
        }
    }
    Ok(Outcome::Passed(
        "each pod verifies under its own host and fails under the Service name".into(),
    ))
}

fn pod_uid(lane: &Lane<'_>, ordinal: usize) -> Result<String> {
    lane.kubectl(&[
        "get",
        "pod",
        "-n",
        NAMESPACE,
        &format!("{RELEASE}-{ordinal}"),
        "-o",
        "jsonpath={.metadata.uid}",
    ])
}

fn restart_survival(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
    values: &Value,
) -> Result<Outcome> {
    let create = json!({"op": ulid::Ulid::generate().to_string(), "input": {"source": "empty", "labels": {"lane": "chart"}}});
    let content = "c3Vydml2ZXMgYSByb2xsaW5nIHJlc3RhcnQ=";
    let before = pod_uid(lane, 0)?;
    let workspace = {
        let forward = Forward::open(lane, 0)?;
        let pod = Target::pod(&forward, client, material, 0);
        let (status, created) = call(&pod, "POST", "/v1/workspaces", Some(&create))?;
        ensure!(
            status == 201,
            "workspace create answered {status}: {created}"
        );
        let workspace = created["result"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let write = json!({"op": ulid::Ulid::generate().to_string(), "input": {"content": {"encoding": "base64", "data": content}}});
        let (status, written) = call(
            &pod,
            "PUT",
            &format!("/v1/workspaces/{workspace}/files/chart-lane.txt"),
            Some(&write),
        )?;
        ensure!(status == 200, "file write answered {status}: {written}");
        workspace
    };

    let rolled = lane.work.join("rolled.json");
    fs::write(
        &rolled,
        serde_json::to_vec_pretty(&chart::merged(
            values,
            &json!({"podAnnotations": {"b10x.dev/lane-roll": "1"}}),
        ))?,
    )?;
    let chart_path = crate::repo::root()?.join(CHART).display().to_string();
    lane.helm(&[
        "upgrade",
        RELEASE,
        &chart_path,
        "-n",
        NAMESPACE,
        "--values",
        &rolled.display().to_string(),
        "--wait",
        "--timeout",
        "6m",
    ])?;
    lane.kubectl(&[
        "rollout",
        "status",
        "-n",
        NAMESPACE,
        &format!("statefulset/{RELEASE}"),
        "--timeout",
        "6m",
    ])?;
    ensure!(
        pod_uid(lane, 0)? != before,
        "the upgrade did not restart {RELEASE}-0"
    );

    let forward = Forward::open(lane, 0)?;
    let pod = Target::pod(&forward, client, material, 0);
    let (status, read) = call(
        &pod,
        "GET",
        &format!(
            "/v1/workspaces/{workspace}/files/chart-lane.txt?mode=file&offset=0&limit_bytes=64"
        ),
        None,
    )?;
    ensure!(
        status == 200 && read["result"]["content"]["data"] == content,
        "the file did not survive the restart: {status} {read}"
    );
    let (status, replayed) = call(&pod, "POST", "/v1/workspaces", Some(&create))?;
    ensure!(
        (200..300).contains(&status) && replayed["result"]["id"] == workspace.as_str(),
        "the replayed create did not answer {workspace}: {status} {replayed}"
    );
    Ok(Outcome::Passed(format!(
        "{workspace} and its file survived a rolling restart; the replayed operation answered it"
    )))
}

fn egress(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
    allowed: &Listener,
    denied: &Listener,
) -> Result<Outcome> {
    let forward = Forward::open(lane, 1)?;
    // The listeners only count connections and never read a request, so a fixed value suffices.
    let pod = Target {
        timeout: Duration::from_secs(45),
        source_authorization: Some("chart-lane-source-authorization"),
        ..Target::pod(&forward, client, material, 1)
    };
    let attempt = |source: &str, listener: &Listener| {
        let body = json!({"op": ulid::Ulid::generate().to_string(), "input": {
            "source": {"git": {
                "source": source,
                "locator": format!("https://{}/repository.git", listener.address),
                "ref": "main",
                "commit": "1".repeat(40),
                "depth": 1
            }},
            "labels": {}
        }});
        https(&pod, "POST", "/v1/workspaces", Some(&body))
    };
    let declared = attempt("declared", allowed);
    if allowed.accepted.load(Ordering::SeqCst) == 0 {
        return Ok(Outcome::Absent(format!(
            "the daemon never dialled the declared Git source at {}, so a silent undeclared one proves nothing: {declared:?}",
            allowed.address
        )));
    }
    let undeclared = attempt("undeclared", denied);
    ensure!(
        denied.accepted.load(Ordering::SeqCst) == 0,
        "a connection to the undeclared {} arrived; the NetworkPolicy admits that host on other ports only",
        denied.address
    );
    let dialled = dial_stage(&undeclared)
        .map_err(|answer| anyhow!("the undeclared Git source was refused before the daemon dialled {}, so its silence proves nothing about the NetworkPolicy: {answer}", denied.address))?;
    Ok(Outcome::Passed(format!(
        "the declared {} was reached; the undeclared {} never was, and the daemon dialled it ({dialled})",
        allowed.address, denied.address
    )))
}

/// The refusals the daemon answers only after it dialled a Git source.
const DIAL_STAGE: [&str; 2] = ["workspace.git-fetch-failed", "workspace.git-fetch-timeout"];

/// The first `error.code` anywhere in an answer: the refusal itself or an operation's outcome.
fn refusal_code(payload: &Value) -> Option<&str> {
    match payload {
        Value::Object(map) => map
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .or_else(|| map.values().find_map(refusal_code)),
        Value::Array(items) => items.iter().find_map(refusal_code),
        _ => None,
    }
}

/// Whether the undeclared request reached the dial stage: the daemon answered a dial-stage
/// refusal, or was still dialling when the lane's own read timed out. `Err` carries the answer
/// that shows it refused earlier.
fn dial_stage(
    answer: &Result<std::result::Result<(u16, Value), rustls::Error>>,
) -> std::result::Result<String, String> {
    match answer {
        Ok(Ok((status, payload))) => match refusal_code(payload) {
            Some(code) if DIAL_STAGE.contains(&code) => Ok(format!("{status} {code}")),
            _ => Err(format!("{status} {payload}")),
        },
        Ok(Err(tls)) => Err(format!("TLS to the daemon failed: {tls}")),
        Err(error) => match error.downcast_ref::<std::io::Error>() {
            Some(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Ok("still dialling when the lane's 45 s read timed out".into())
            }
            _ => Err(format!("{error:#}")),
        },
    }
}

fn pod_security(lane: &Lane<'_>, chart_dir: &Path, values: &Value) -> Result<Outcome> {
    let execution = chart::merged(values, &json!({"execution": {"enabled": true}}));
    let rendered = chart::render(&lane.args.helm, chart_dir, RELEASE, NAMESPACE, &execution)?
        .map_err(|stderr| anyhow!("helm template with execution: {stderr}"))?;
    let statefulset = chart_yaml::documents(&rendered)?
        .into_iter()
        .find(|document| document["kind"] == "StatefulSet")
        .ok_or_else(|| anyhow!("no StatefulSet rendered"))?;
    let mut spec = statefulset["spec"]["template"]["spec"].clone();
    spec["serviceAccountName"] = json!("default");
    spec["volumes"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("the pod has no volumes"))?
        .push(json!({"name": "ledger", "emptyDir": {}}));
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {"name": "pod-security-probe", "labels": statefulset["spec"]["template"]["metadata"]["labels"]},
        "spec": spec
    });
    let file = lane.work.join("pod-security-probe.json");
    fs::write(&file, serde_json::to_vec_pretty(&pod)?)?;
    let file = file.display().to_string();
    lane.kubectl(&[
        "create",
        "--dry-run=server",
        "-n",
        PRIVILEGED_NAMESPACE,
        "-f",
        &file,
        "-o",
        "name",
    ])
    .context("the privileged namespace refused the execution-enabled pod")?;
    match lane.kubectl(&[
        "create",
        "--dry-run=server",
        "-n",
        BASELINE_NAMESPACE,
        "-f",
        &file,
        "-o",
        "name",
    ]) {
        Ok(_) => bail!("the baseline namespace admitted the execution-enabled pod"),
        Err(error) if format!("{error:#}").contains("PodSecurity") => {}
        Err(error) => bail!("the baseline namespace refused for another reason: {error:#}"),
    }
    Ok(Outcome::Passed(
        "execution admitted under privileged, refused under baseline; execution off runs under restricted"
            .into(),
    ))
}

fn uninstall(lane: &Lane<'_>) -> Result<Outcome> {
    lane.helm(&[
        "uninstall",
        RELEASE,
        "-n",
        NAMESPACE,
        "--wait",
        "--timeout",
        "3m",
    ])?;
    let deadline = Instant::now() + Duration::from_mins(2);
    loop {
        let left = lane.kubectl(&[
            "get",
            "statefulsets,services,networkpolicies,serviceaccounts,configmaps,pods",
            "-n",
            NAMESPACE,
            "-l",
            "app.kubernetes.io/instance=lane",
            "-o",
            "name",
        ])?;
        if left.trim().is_empty() {
            break;
        }
        ensure!(Instant::now() < deadline, "uninstall left {left}");
        thread::sleep(Duration::from_secs(2));
    }
    let claims: BTreeSet<String> = lane
        .kubectl(&["get", "pvc", "-n", NAMESPACE, "-o", "name"])?
        .lines()
        .map(str::to_owned)
        .collect();
    let wanted: BTreeSet<String> = (0..REPLICAS)
        .map(|ordinal| format!("persistentvolumeclaim/ledger-{RELEASE}-{ordinal}"))
        .collect();
    ensure!(
        claims == wanted,
        "after uninstall the claims are {claims:?}, not {wanted:?}"
    );
    Ok(Outcome::Passed(format!("only {wanted:?} remain")))
}

/// Design 31 § 7's step: the clean-room runner once against ordinal 0 over a port-forward,
/// verified under the pod's own name and refused under the headless Service's, with the stub's
/// credential over an inherited socket. No restart hook, so the restart group is absent
/// (`input-not-declared`); the absent set goes to the job summary, unpinned, because cluster
/// facts vary.
fn clean_room_run(lane: &Lane<'_>, material: &Material) -> Result<Outcome> {
    let runner = lane.args.conformance_runner.as_deref().ok_or_else(|| {
        anyhow!(
            "--conformance-runner was not given: the workflow exports it from the Dockerfile's \
             conformance-runner stage"
        )
    })?;
    let out = lane
        .args
        .conformance_out
        .clone()
        .unwrap_or_else(|| lane.work.join("conformance-report"));
    let roots = lane.work.join("conformance-roots.pem");
    fs::write(&roots, &material.daemon_pem)?;
    let log = lane.work.join("conformance.log");
    let forward = Forward::open(lane, 0)?;
    let call = clean_room::Call {
        endpoint: format!("https://127.0.0.1:{}/", forward.1),
        trust_roots: &roots,
        server_name: &material.pod_names[0],
        forbidden_name: &material.service_name,
        restart_hook: None,
        ops_origin: None,
        out: &out,
        deadline: Duration::from_mins(5),
        io_timeout: Duration::from_secs(30),
        restart_timeout: Duration::from_mins(1),
    };
    let token = clean_room::TokenSocket::serve(material.credential.clone())?;
    let mut running = clean_room::spawn(runner, &call, &token, &log)?;
    let exit = running.wait(call.bound(), lane.stop)?;
    drop(forward);
    let answered = token.finish();
    let tally = clean_room::judge(exit, &out, &log, &material.credential, None)?;
    println!("{}", clean_room::summary(&tally));
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?;
        file.write_all(clean_room::summary(&tally).as_bytes())?;
    }
    Ok(Outcome::Passed(format!(
        "{}; {answered} credential request(s) answered",
        clean_room::evidence(&tally)
    )))
}

/// Design 31's clean-room runner as both cluster lanes run it: built with `-p` and nothing else,
/// handed the runner's end of a token socket no other child inherits, bounded, and judged by its
/// exit code and its report, neither of which may carry the credential.
pub(crate) mod clean_room {
    use std::fmt::Write as _;
    use std::fs;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt as _;
    use std::path::Path;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, anyhow, bail};
    use serde_json::Value;

    /// The runner's package; a lane builds it with `-p` and nothing else (design 31 § 2).
    pub const PACKAGE: &str = "b10x-substrate-conformance";
    /// What a provider answers for a purpose it does not serve (design 31 § 5).
    const UNSUPPORTED: &str = "unsupported";
    /// The absences the endpoint's own facts or the runner's bounds decide, which a lane accepts
    /// whatever inputs it declares.
    const DECIDED_BY_THE_ENDPOINT: [&str; 3] =
        ["fact-absent", "fact-unsatisfied", "bound-exceeded"];
    /// The credential format's fixed prefix (ADR 0026); its 43-character body follows it.
    const CREDENTIAL_PREFIX: &str = "identity_access_v1_";

    /// One run's arguments, apart from the token descriptor.
    pub struct Call<'a> {
        pub endpoint: String,
        pub trust_roots: &'a Path,
        pub server_name: &'a str,
        pub forbidden_name: &'a str,
        /// The hook's absolute path and its arguments, run argv-only by the runner.
        pub restart_hook: Option<(&'a str, Vec<String>)>,
        pub ops_origin: Option<String>,
        pub out: &'a Path,
        pub deadline: Duration,
        pub io_timeout: Duration,
        pub restart_timeout: Duration,
    }

    impl Call<'_> {
        /// How long a lane waits for the runner before it kills it: the runner's own deadline,
        /// its restart bound and a minute to write the report.
        pub fn bound(&self) -> Duration {
            self.deadline + self.restart_timeout + Duration::from_mins(1)
        }
    }

    /// The runner's arguments, one element each, with `--token-fd <descriptor>`.
    pub fn argv(call: &Call<'_>, descriptor: i32) -> Vec<String> {
        let mut argv: Vec<String> = [
            ("--endpoint", call.endpoint.clone()),
            ("--trust-roots", call.trust_roots.display().to_string()),
            ("--server-name", call.server_name.to_owned()),
            ("--forbidden-name", call.forbidden_name.to_owned()),
            ("--token-fd", descriptor.to_string()),
            ("--out", call.out.display().to_string()),
            ("--deadline", call.deadline.as_secs().to_string()),
            ("--io-timeout", call.io_timeout.as_secs().to_string()),
            (
                "--restart-timeout",
                call.restart_timeout.as_secs().to_string(),
            ),
        ]
        .into_iter()
        .flat_map(|(flag, value)| [flag.to_owned(), value])
        .collect();
        if let Some((hook, arguments)) = &call.restart_hook {
            argv.extend(["--restart-hook".to_owned(), (*hook).to_owned()]);
            for argument in arguments {
                argv.extend(["--restart-hook-arg".to_owned(), argument.clone()]);
            }
        }
        if let Some(origin) = &call.ops_origin {
            argv.extend(["--ops-origin".to_owned(), origin.clone()]);
        }
        argv
    }

    /// The answer to one credential request line: `credential` for the primary purpose and
    /// `unsupported` for anything else, the narrowed purpose and an unreadable line included.
    pub fn answer<'c>(request: &str, credential: &'c str) -> &'c str {
        let purpose = serde_json::from_str::<Value>(request)
            .ok()
            .and_then(|request| request["purpose"].as_str().map(str::to_owned));
        if purpose.as_deref() == Some("primary") {
            credential
        } else {
            UNSUPPORTED
        }
    }

    /// The lane's token provider: a Unix stream pair whose far end only the runner inherits,
    /// served from a thread until every copy of that end is closed.
    pub struct TokenSocket {
        theirs: UnixStream,
        server: thread::JoinHandle<usize>,
    }

    impl TokenSocket {
        /// Serves `credential` for the primary purpose and `unsupported` for any other.
        ///
        /// # Errors
        ///
        /// The pair could not be made.
        pub fn serve(credential: String) -> Result<Self> {
            let (ours, theirs) = UnixStream::pair().context("the token socket pair")?;
            let reading = ours.try_clone().context("the token socket")?;
            let server = thread::spawn(move || {
                let mut reader = BufReader::new(reading);
                let mut writer = ours;
                let mut line = String::new();
                let mut answered = 0;
                while reader.read_line(&mut line).is_ok_and(|count| count > 0) {
                    if writeln!(writer, "{}", answer(&line, &credential)).is_err() {
                        break;
                    }
                    answered += 1;
                    line.clear();
                }
                answered
            });
            Ok(Self { theirs, server })
        }

        /// Makes `command`'s child, and only it, inherit the runner's end: the descriptor keeps
        /// close-on-exec in this process and loses it in the child between fork and exec. The
        /// descriptor number the runner is to read.
        fn hand_to(&self, command: &mut Command) -> i32 {
            let descriptor = self.theirs.as_raw_fd();
            // SAFETY: the closure runs in the forked child before exec and calls only
            // `fcntl(2)`, which is async-signal-safe, on a descriptor the child inherited.
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(descriptor, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            descriptor
        }

        /// Closes this process's copy of the runner's end and waits for the server: how many
        /// requests it answered.
        pub fn finish(self) -> usize {
            drop(self.theirs);
            self.server.join().unwrap_or(0)
        }
    }

    /// The runner while it runs; killed and reaped if it is dropped before it ended.
    pub struct Running {
        child: Child,
        ended: Option<ExitStatus>,
    }

    impl Running {
        /// Its exit status once it ended, reaped.
        ///
        /// # Errors
        ///
        /// The process could not be waited on.
        pub fn exited(&mut self) -> Result<Option<ExitStatus>> {
            if self.ended.is_none() {
                self.ended = self.child.try_wait()?;
            }
            Ok(self.ended)
        }

        /// Kills and reaps it, unless it already ended.
        pub fn kill(&mut self) {
            if self.ended.is_none() {
                let _ = self.child.kill();
                self.ended = self.child.wait().ok();
            }
        }

        /// Waits for its exit within `bound`, killing it on overrun or once `stop` is set: its
        /// exit code, or `None` when it was killed or a signal ended it.
        ///
        /// # Errors
        ///
        /// The process could not be waited on.
        pub fn wait(&mut self, bound: Duration, stop: &AtomicBool) -> Result<Option<i32>> {
            let deadline = Instant::now() + bound;
            loop {
                if let Some(status) = self.exited()? {
                    return Ok(status.code());
                }
                if stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    self.kill();
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(200));
            }
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.kill();
        }
    }

    /// Starts `runner` with an empty environment, no standard input, its output in `log`, and
    /// the token socket's far end.
    ///
    /// # Errors
    ///
    /// The log could not be created or the runner did not start.
    pub fn spawn(
        runner: &Path,
        call: &Call<'_>,
        token: &TokenSocket,
        log: &Path,
    ) -> Result<Running> {
        let log = fs::File::create(log)?;
        let mut command = Command::new(runner);
        let descriptor = token.hand_to(&mut command);
        command
            .args(argv(call, descriptor))
            .env_clear()
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        let child = command
            .spawn()
            .with_context(|| format!("starting {}", runner.display()))?;
        Ok(Running { child, ended: None })
    }

    /// What a lane keeps from a passed run.
    #[derive(Debug, PartialEq, Eq)]
    pub struct Tally {
        pub passed: u64,
        /// Each absent case and its reason, in the report's order.
        pub absent: Vec<(String, String)>,
    }

    fn id(case: &Value) -> String {
        case["id"].as_str().unwrap_or("<no id>").to_owned()
    }

    fn reason(case: &Value) -> &str {
        case["absent"]["reason"].as_str().unwrap_or("<no reason>")
    }

    fn accepted(case: &Value, undeclared: &[&str]) -> bool {
        let reason = reason(case);
        if DECIDED_BY_THE_ENDPOINT.contains(&reason) {
            return true;
        }
        reason == "input-not-declared"
            && case["absent"]["inputs"].as_array().is_some_and(|inputs| {
                !inputs.is_empty()
                    && inputs.iter().all(|input| {
                        input
                            .as_str()
                            .is_some_and(|input| undeclared.contains(&input))
                    })
            })
    }

    /// Whether the runner's exit code and report certify the endpoint for a lane: exit 0, the
    /// verdict `passed` and no failed case. Absence never satisfies a core group (design 31 § 3),
    /// so an exit 4 is refused with its absent cases named.
    ///
    /// `undeclared` is `None` when the lane accepts every absence the runner reports. Otherwise
    /// it names the inputs the lane does not declare, and an absence is accepted only when the
    /// endpoint's facts or the runner's bounds decided it, or when it names only those inputs.
    ///
    /// # Errors
    ///
    /// Why not, naming the failed and the refused absent cases.
    pub fn verdict(
        exit: Option<i32>,
        report: Option<&Value>,
        undeclared: Option<&[&str]>,
    ) -> Result<Tally, String> {
        let code = exit.ok_or(
            "the runner did not exit within its bound or a signal ended it, so it wrote no verdict",
        )?;
        let named = match code {
            0 => "passed",
            1 => "failed",
            4 => "incomplete",
            2 => {
                return Err(
                    "the runner refused its input or the endpoint's contract (exit 2)".into(),
                );
            }
            3 => return Err("the runner's harness failed (exit 3)".into()),
            other => return Err(format!("the runner exited {other}")),
        };
        let report =
            report.ok_or_else(|| format!("the runner exited {code} and wrote no report"))?;
        let stated = report["verdict"].as_str().unwrap_or("<none>");
        if stated != named {
            return Err(format!(
                "the runner exited {code}, but its report's verdict is {stated}"
            ));
        }
        let cases = report["cases"]
            .as_array()
            .ok_or("the report lists no cases")?;
        let failed: Vec<String> = cases
            .iter()
            .filter(|case| case["status"] == "failed")
            .map(|case| {
                let diagnostics: Vec<&str> = case["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                format!("{} ({})", id(case), diagnostics.join(", "))
            })
            .collect();
        let absent: Vec<&Value> = cases
            .iter()
            .filter(|case| case["status"] == "absent")
            .collect();
        let named_absent = |cases: &[&Value]| {
            cases
                .iter()
                .map(|case| format!("{} ({})", id(case), reason(case)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if named != "passed" || !failed.is_empty() {
            return Err(format!(
                "verdict {named}; failed: {}; absent: {}",
                failed.join(", "),
                named_absent(&absent)
            ));
        }
        if let Some(undeclared) = undeclared {
            let refused: Vec<&Value> = absent
                .iter()
                .copied()
                .filter(|case| !accepted(case, undeclared))
                .collect();
            if !refused.is_empty() {
                return Err(format!(
                    "absent for a reason this lane does not accept: {}",
                    named_absent(&refused)
                ));
            }
        }
        let passed = report["totals"]["passed"]
            .as_u64()
            .ok_or("the report states no passed total")?;
        Ok(Tally {
            passed,
            absent: absent
                .iter()
                .map(|case| (id(case), reason(case).to_owned()))
                .collect(),
        })
    }

    /// Whether `bytes` hold `credential`, its body after [`CREDENTIAL_PREFIX`], or the prefix
    /// itself anywhere (design 31 § 5 point 4). An empty credential is found nowhere.
    pub fn carries(bytes: &[u8], credential: &str) -> bool {
        let holds = |needle: &str| {
            !needle.is_empty()
                && bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
        };
        !credential.is_empty()
            && (holds(credential)
                || holds(credential.strip_prefix(CREDENTIAL_PREFIX).unwrap_or(""))
                || holds(CREDENTIAL_PREFIX))
    }

    /// Reads the run's report, digest and output, refuses any that holds `credential`, and
    /// judges the run with [`verdict`]. A refused run's report, digest and output are deleted
    /// first, so a later step that uploads the report directory (`chart-lane.yml` uploads it
    /// whatever the outcome) finds nothing that holds the credential.
    ///
    /// # Errors
    ///
    /// A file holds the credential (nothing of it is shown; the files are deleted, and a file
    /// that could not be deleted is named), the report is not JSON, or
    /// [`verdict`] refused the run; the runner's last lines are appended, as it prints only case
    /// ids, statuses and closed codes.
    pub fn judge(
        exit: Option<i32>,
        out: &Path,
        log: &Path,
        credential: &str,
        undeclared: Option<&[&str]>,
    ) -> Result<Tally> {
        let report = fs::read(out.join("report.json")).ok();
        let digest = fs::read(out.join("report.json.sha256")).ok();
        let printed = fs::read(log).unwrap_or_default();
        if [
            report.as_deref(),
            digest.as_deref(),
            Some(printed.as_slice()),
        ]
        .into_iter()
        .flatten()
        .any(|bytes| carries(bytes, credential))
        {
            let kept: Vec<String> = [
                out.join("report.json"),
                out.join("report.json.sha256"),
                log.to_path_buf(),
            ]
            .iter()
            .filter(|path| match fs::remove_file(path) {
                Ok(()) => false,
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
            })
            .map(|path| path.display().to_string())
            .collect();
            if kept.is_empty() {
                bail!(
                    "the runner's report or output holds the credential; neither is shown, and \
                     both were deleted"
                );
            }
            bail!(
                "the runner's report or output holds the credential; neither is shown, and {} \
                 could not be deleted",
                kept.join(", ")
            );
        }
        let report: Option<Value> = report
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .context("report.json is not JSON")?;
        verdict(exit, report.as_ref(), undeclared).map_err(|why| {
            let printed = String::from_utf8_lossy(&printed);
            let lines: Vec<&str> = printed.lines().collect();
            anyhow!(
                "{why}; the runner's last lines: {}",
                lines[lines.len().saturating_sub(6)..].join(" | ")
            )
        })
    }

    /// The step's evidence: the totals and every absent case with its reason.
    pub fn evidence(tally: &Tally) -> String {
        let absent: Vec<String> = tally
            .absent
            .iter()
            .map(|(id, reason)| format!("{id} ({reason})"))
            .collect();
        format!(
            "verdict passed, {} passed, {} absent: {}",
            tally.passed,
            tally.absent.len(),
            absent.join(", ")
        )
    }

    /// The job summary's block: the totals and the absent set, unpinned (design 31 § 7).
    pub fn summary(tally: &Tally) -> String {
        let mut text = format!(
            "### Clean-room runner\n\nverdict passed: {} passed, {} absent\n\n",
            tally.passed,
            tally.absent.len()
        );
        for (id, reason) in &tally.absent {
            let _ = writeln!(text, "- `{id}`: {reason}");
        }
        text
    }

    #[cfg(test)]
    mod tests {
        use std::io::{BufRead as _, BufReader, Write as _};
        use std::os::unix::net::UnixStream;
        use std::path::Path;
        use std::time::Duration;

        use serde_json::{Value, json};

        use super::{Call, Tally, TokenSocket, answer, argv, carries, verdict};

        fn report(verdict: &str, cases: &Value) -> Value {
            json!({"verdict": verdict, "totals": {"passed": 2, "failed": 0, "absent": 1}, "cases": cases})
        }

        fn case(id: &str, status: &str, absent: Option<Value>) -> Value {
            let mut case = json!({"id": id, "group": "g", "status": status, "diagnostics": []});
            if let Some(absent) = absent {
                case["absent"] = absent;
            }
            case
        }

        #[test]
        fn the_argv_carries_each_value_as_one_argument_and_the_hook_argv_only() {
            let call = Call {
                endpoint: "https://127.0.0.1:4443/".to_owned(),
                trust_roots: Path::new("/w/roots.pem"),
                server_name: "lane-0.lane.ns.svc.cluster.local",
                forbidden_name: "lane.ns.svc.cluster.local",
                restart_hook: Some(("/usr/bin/kill", vec!["-KILL".to_owned(), "41".to_owned()])),
                ops_origin: Some("http://127.0.0.1:9090".to_owned()),
                out: Path::new("/w/out"),
                deadline: Duration::from_mins(30),
                io_timeout: Duration::from_mins(1),
                restart_timeout: Duration::from_mins(10),
            };
            assert_eq!(
                argv(&call, 7),
                [
                    "--endpoint",
                    "https://127.0.0.1:4443/",
                    "--trust-roots",
                    "/w/roots.pem",
                    "--server-name",
                    "lane-0.lane.ns.svc.cluster.local",
                    "--forbidden-name",
                    "lane.ns.svc.cluster.local",
                    "--token-fd",
                    "7",
                    "--out",
                    "/w/out",
                    "--deadline",
                    "1800",
                    "--io-timeout",
                    "60",
                    "--restart-timeout",
                    "600",
                    "--restart-hook",
                    "/usr/bin/kill",
                    "--restart-hook-arg",
                    "-KILL",
                    "--restart-hook-arg",
                    "41",
                    "--ops-origin",
                    "http://127.0.0.1:9090",
                ]
            );
            assert_eq!(call.bound(), Duration::from_mins(41));
            let bare = Call {
                restart_hook: None,
                ops_origin: None,
                ..call
            };
            let bare = argv(&bare, 7);
            assert!(
                !bare
                    .iter()
                    .any(|argument| argument.starts_with("--restart-hook"))
            );
            assert!(!bare.iter().any(|argument| argument == "--ops-origin"));
            assert!(
                !bare
                    .iter()
                    .any(|argument| argument.contains("identity_access"))
            );
        }

        #[test]
        fn only_the_primary_purpose_is_answered_with_the_credential() {
            let credential = "identity_access_v1_secret";
            let primary = r#"{"reason":"request","purpose":"primary","scopes":["exec"]}"#;
            assert_eq!(answer(primary, credential), credential);
            for other in [
                r#"{"reason":"request","purpose":"narrowed","scopes":["observe"]}"#,
                r#"{"purpose":"Primary"}"#,
                "primary",
                "",
            ] {
                assert_eq!(answer(other, credential), "unsupported", "{other}");
            }
        }

        #[test]
        fn the_token_socket_answers_each_line_and_ends_when_the_far_end_closes() {
            let token = TokenSocket::serve("identity_access_v1_secret".to_owned()).expect("pair");
            let far: UnixStream = token.theirs.try_clone().expect("far end");
            let mut writer = far.try_clone().expect("writer");
            let mut reader = BufReader::new(far);
            let mut line = String::new();
            writeln!(
                writer,
                r#"{{"reason":"request","purpose":"primary","scopes":[]}}"#
            )
            .expect("request");
            reader.read_line(&mut line).expect("answer");
            assert_eq!(line, "identity_access_v1_secret\n");
            line.clear();
            writeln!(
                writer,
                r#"{{"reason":"request","purpose":"narrowed","scopes":[]}}"#
            )
            .expect("request");
            reader.read_line(&mut line).expect("answer");
            assert_eq!(line, "unsupported\n");
            drop((writer, reader));
            assert_eq!(token.finish(), 2);
        }

        #[test]
        fn only_exit_zero_with_verdict_passed_and_no_failed_case_certifies() {
            let cases = json!([
                case("machine.envelope", "passed", None),
                case("tls.v12-refused", "passed", None),
                case(
                    "exec.run-output",
                    "absent",
                    Some(
                        json!({"reason": "fact-absent", "facts": ["exec.no-egress"], "inputs": [], "scopes": []})
                    )
                ),
            ]);
            assert_eq!(
                verdict(Some(0), Some(&report("passed", &cases)), Some(&[])),
                Ok(Tally {
                    passed: 2,
                    absent: vec![("exec.run-output".to_owned(), "fact-absent".to_owned())],
                })
            );
            for (exit, stated, why) in [
                (None, "passed", "did not exit"),
                (Some(2), "passed", "exit 2"),
                (Some(3), "passed", "exit 3"),
                (Some(9), "passed", "exited 9"),
                (Some(0), "failed", "verdict is failed"),
                (Some(1), "passed", "verdict is passed"),
            ] {
                let refused = verdict(exit, Some(&report(stated, &cases)), None).expect_err(why);
                assert!(refused.contains(why), "{refused}");
            }
            assert!(
                verdict(Some(0), None, None)
                    .expect_err("no report")
                    .contains("wrote no report")
            );
            let incomplete = json!([case(
                "recovery.dropped",
                "absent",
                Some(json!({"reason": "fact-absent", "facts": ["x"], "inputs": [], "scopes": []}))
            ),]);
            let refused = verdict(Some(4), Some(&report("incomplete", &incomplete)), None)
                .expect_err("incomplete");
            assert!(
                refused.contains("recovery.dropped (fact-absent)"),
                "{refused}"
            );
            let failed = json!([
                {"id": "auth.absent", "group": "auth", "status": "failed", "diagnostics": ["status-unexpected"]},
            ]);
            let refused =
                verdict(Some(1), Some(&report("failed", &failed)), None).expect_err("failed");
            assert!(
                refused.contains("auth.absent (status-unexpected)"),
                "{refused}"
            );
        }

        #[test]
        fn a_lane_accepts_only_absences_its_endpoint_decided_or_it_left_undeclared() {
            let absent = |reason: &str, inputs: Value| {
                case(
                    "c",
                    "absent",
                    Some(json!({"reason": reason, "facts": [], "inputs": inputs, "scopes": []})),
                )
            };
            let narrowed = &["narrowed-credential"][..];
            for (case, accepted) in [
                (absent("fact-absent", json!([])), true),
                (absent("fact-unsatisfied", json!([])), true),
                (absent("bound-exceeded", json!([])), true),
                (
                    absent("input-not-declared", json!(["narrowed-credential"])),
                    true,
                ),
                (absent("input-not-declared", json!(["restart-hook"])), false),
                (
                    absent(
                        "input-not-declared",
                        json!(["narrowed-credential", "ops-origin"]),
                    ),
                    false,
                ),
                (absent("input-not-declared", json!([])), false),
                (absent("scope-not-declared", json!([])), false),
            ] {
                let report = report("passed", &json!([case]));
                assert_eq!(
                    verdict(Some(0), Some(&report), Some(narrowed)).is_ok(),
                    accepted,
                    "{case}"
                );
                assert!(verdict(Some(0), Some(&report), None).is_ok(), "{case}");
            }
        }

        #[test]
        fn a_credential_is_found_anywhere_in_the_bytes_and_an_empty_one_nowhere() {
            // The security review on u1 widened the match to the body and the prefix (design 31
            // § 5 point 4), so the former case "the bare prefix is not the credential" is
            // replaced by the cases below with a credential in the bundle's format.
            let body = "Ab3-_".repeat(8) + "xyz";
            let credential = format!("identity_access_v1_{body}");
            assert!(carries(format!("x {credential} y").as_bytes(), &credential));
            assert!(carries(credential.as_bytes(), &credential));
            assert!(carries(format!("v{body}").as_bytes(), &credential));
            assert!(carries(b"driver identity_access_v1_", &credential));
            assert!(!carries(&body.as_bytes()[1..], &credential));
            assert!(!carries(b"identity_access_v2_", &credential));
            assert!(!carries(b"anything", ""));
            assert!(!carries(b"identity_access_v1_", ""));
        }

        /// Security review (u1): `chart-lane.yml` uploads the report directory whatever the step's
        /// outcome, so a run refused for holding the credential leaves none of its files behind.
        #[test]
        fn a_refused_run_leaves_no_file_that_holds_the_credential() {
            let directory = tempfile::tempdir().expect("scratch");
            let out = directory.path().join("out");
            std::fs::create_dir(&out).expect("out");
            let log = directory.path().join("conformance.log");
            let credential = format!("identity_access_v1_{}", "q".repeat(43));
            std::fs::write(
                out.join("report.json"),
                format!("{{\"d\":\"{}\"}}", "q".repeat(43)),
            )
            .expect("report");
            std::fs::write(out.join("report.json.sha256"), "0").expect("digest");
            std::fs::write(&log, "case a passed\n").expect("log");
            let refused = super::judge(Some(1), &out, &log, &credential, None)
                .expect_err("the report holds the body");
            assert!(!refused.to_string().contains(&"q".repeat(43)));
            assert!(!out.join("report.json").exists());
            assert!(!out.join("report.json.sha256").exists());
            assert!(!log.exists());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diagnosis_sandbox_runs_as_the_daemon_and_binds_a_tmp_directory() {
        let uid = format!("--reuid={DAEMON_UID}");
        let gid = format!("--regid={DAEMON_UID}");
        for wanted in [
            uid.as_str(),
            gid.as_str(),
            "--clear-groups",
            "--inh-caps=-all",
            "--bounding-set=-all",
            "--no-new-privs",
        ] {
            assert!(DIAGNOSIS_SANDBOX.contains(&wanted), "{wanted}");
        }
        let script = DIAGNOSIS_SANDBOX[DIAGNOSIS_SANDBOX.len() - 1];
        for wanted in [
            "d=$(mktemp -d)",
            "--unshare-user --disable-userns",
            "--assert-userns-disabled",
            "--ro-bind \"$d\" /runtime/sentinel",
        ] {
            assert!(script.contains(wanted), "{wanted} not in {script}");
        }
    }

    #[test]
    fn only_the_runs_own_profile_denials_are_reported() {
        let profile = "substrate-host-exec-v1-lane-01m62nkn5vqh7jba8ym0z7js91";
        let denied = format!(
            "[4279684.727023] audit: type=1400 audit(1793238653.480:570): apparmor=\"DENIED\" \
             operation=\"mount\" class=\"mount\" info=\"failed flags match\" error=-13 \
             profile=\"{profile}\" name=\"/newroot/runtime/sentinel/\" pid=789774 comm=\"bwrap\" \
             flags=\"ro, nosuid, nodev, remount, noatime, bind, silent\""
        );
        let log = [
            denied.clone(),
            denied.replace(profile, &format!("{profile}x")),
            denied.replace("DENIED", "ALLOWED"),
            "[1.0] unrelated".to_owned(),
        ]
        .join("\n");
        assert_eq!(apparmor_denials(&log, profile), [denied.as_str()]);
    }

    #[test]
    fn every_execution_enabled_scenario_is_reported_absent_by_name_on_every_run() {
        let outcomes = vec![
            ("placement".to_owned(), Outcome::Passed("ok".into())),
            (
                "egress".to_owned(),
                Outcome::Absent("never dialled\nthe declared source".into()),
            ),
        ];
        let absent = absent_scenarios(&outcomes);
        let names: Vec<&str> = absent.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "egress",
                "execution bootstrap",
                "Localhost seccomp profile",
                "Localhost AppArmor profile",
                "TLS key after the uid drop"
            ]
        );
        let lines = absent_annotations(&absent);
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0],
            "::notice title=chart-lane absent::egress: never dialled%0Athe declared source"
        );
        assert!(lines.iter().all(|line| !line.contains('\n')));
        assert!(
            lines[1].starts_with("::notice title=chart-lane absent::execution bootstrap: "),
            "{}",
            lines[1]
        );
        assert_eq!(
            absent_list(&absent_scenarios(&[])),
            "execution bootstrap; Localhost seccomp profile; Localhost AppArmor profile; TLS key after the uid drop"
        );
        assert_eq!(SCENARIOS.len(), 4);
    }

    #[test]
    fn the_declared_scenarios_are_one_literal_array_naming_every_scenario_once() {
        let source = include_str!("chart_cluster.rs");
        assert_eq!(declared_scenarios(source), Some(SCENARIOS.len()));
        let names: BTreeSet<&str> = SCENARIOS.iter().copied().collect();
        assert_eq!(names.len(), SCENARIOS.len(), "a scenario is declared twice");
        // A local record covers exactly the execution scenarios the hosted lane reports absent;
        // the seven hosted ones are the hosted lane's, on two workers.
        let execution: Vec<&str> = EXECUTION_ABSENT.iter().map(|(name, _)| *name).collect();
        assert_eq!(SCENARIOS.as_slice(), execution.as_slice());
        for name in HOSTED {
            assert!(!names.contains(name), "{name} is a hosted scenario");
        }
        let array = |entries: &str, length: usize| {
            format!("const X: u8 = 0;\npub const SCENARIOS: [&str; {length}] = [\n{entries}];\n")
        };
        assert_eq!(
            declared_scenarios(&array("    \"a\",\n    \"b c\",\n", 2)),
            Some(2)
        );
        for malformed in [
            array("    \"a\", \"b\",\n", 2),
            array("    \"a\",\n", 2),
            array("    \"a\",\n    // b\n", 1),
            "pub const SCENARIOS: [&str; 0] = [];\n".to_owned(),
            String::new(),
        ] {
            assert_eq!(declared_scenarios(&malformed), None, "{malformed}");
        }
    }

    fn every(passed: bool) -> Vec<(String, Outcome)> {
        SCENARIOS
            .iter()
            .map(|name| {
                let outcome = if passed {
                    Outcome::Passed("ok".into())
                } else {
                    Outcome::Absent("not proven".into())
                };
                ((*name).to_owned(), outcome)
            })
            .collect()
    }

    #[test]
    fn only_a_local_run_that_passed_every_declared_scenario_is_a_record() {
        assert_eq!(local_verdict(&every(true), &[]), Ok(SCENARIOS.len()));
        let error = local_verdict(&every(true), &["egress: refused".to_owned()]).unwrap_err();
        assert!(error.contains("egress: refused"), "{error}");
        let error = local_verdict(&every(false), &[]).unwrap_err();
        assert!(error.contains("absent"), "{error}");
        let mut missing = every(true);
        missing.retain(|(name, _)| name != "Localhost AppArmor profile");
        let error = local_verdict(&missing, &[]).unwrap_err();
        assert!(error.contains("Localhost AppArmor profile"), "{error}");
        let mut doubled = every(true);
        doubled.push((
            "execution bootstrap".to_owned(),
            Outcome::Passed("again".into()),
        ));
        assert!(local_verdict(&doubled, &[]).is_err());
        for undeclared in ["undeclared", "placement"] {
            let mut unknown = every(true);
            unknown.push((undeclared.to_owned(), Outcome::Passed("ok".into())));
            assert!(local_verdict(&unknown, &[]).is_err(), "{undeclared}");
        }
    }

    #[test]
    fn a_record_needs_an_unmoved_head_and_a_checkout_clean_before_and_after() {
        let sha = "a".repeat(40);
        let moved = "b".repeat(40);
        assert_eq!(
            decision(Ok(10), &sha, &sha, true, true),
            Decision::Record(10)
        );
        assert!(matches!(
            decision(Ok(10), &sha, &moved, true, true),
            Decision::NoRecord(reason) if reason.contains("HEAD moved")
        ));
        for (before, after) in [(false, true), (true, false), (false, false)] {
            assert!(matches!(
                decision(Ok(10), &sha, &sha, before, after),
                Decision::NoRecord(reason) if reason.contains("not clean")
            ));
        }
        assert!(matches!(
            decision(Err("egress failed".into()), &sha, &sha, true, true),
            Decision::Failed(reason) if reason.contains("egress failed")
        ));
    }

    #[test]
    fn a_missing_prerequisite_is_refused_by_name_with_every_scenario_that_did_not_run() {
        let message = refusal("AppArmor securityfs is not mounted in substrate-chart-lane-worker");
        assert!(
            message.contains("AppArmor securityfs is not mounted"),
            "{message}"
        );
        for name in SCENARIOS {
            assert!(message.contains(name), "{name} is not named: {message}");
        }
        assert!(!message.contains("Chart-lane:"), "{message}");
    }

    #[test]
    fn the_image_must_carry_the_checkouts_own_revision() {
        let sha = "c".repeat(40);
        assert_eq!(revision_verdict(&sha, &sha), Ok(()));
        for label in ["", "unknown", "<no value>", &"d".repeat(40), &sha[..12]] {
            let error = revision_verdict(label, &sha).unwrap_err();
            assert!(error.contains(REVISION_LABEL), "{error}");
            assert!(error.contains(&sha), "{error}");
        }
    }

    #[test]
    fn the_record_is_the_form_the_release_workflow_accepts() {
        let at = UNIX_EPOCH + Duration::from_hours(497_986);
        assert_eq!(
            evidence_line("abc", 10, at),
            "Chart-lane: abc 10 2026-10-23T10:00:00Z"
        );
    }

    #[test]
    fn a_profile_already_in_the_shared_kernel_is_found_by_its_exact_name() {
        let listing = "docker-default (enforce)\nsubstrate-host-exec-v2 (enforce)\n";
        assert!(profile_loaded(listing, APPARMOR_PROFILE));
        assert!(profile_loaded(
            "substrate-host-exec-v2 (complain)\n",
            APPARMOR_PROFILE
        ));
        assert!(!profile_loaded(
            "docker-default (enforce)\n",
            APPARMOR_PROFILE
        ));
        assert!(!profile_loaded(
            "substrate-host-exec-v20 (enforce)\nx//substrate-host-exec-v2 (enforce)\n",
            APPARMOR_PROFILE
        ));
        assert!(!profile_loaded(
            "substrate-host-exec-v1 (enforce)\n",
            APPARMOR_PROFILE
        ));
        assert!(profile_loaded(
            "substrate-host-exec-v1 (enforce)\n",
            "substrate-host-exec-v1"
        ));
        assert!(!profile_loaded("", APPARMOR_PROFILE));
    }

    #[test]
    fn local_mode_runs_one_worker_and_loads_the_hosts_securityfs_into_it_only() {
        let hosted = kind_config(false);
        assert!(!hosted.contains("extraMounts"), "{hosted}");
        assert_eq!(hosted.matches("role: worker").count(), REPLICAS, "{hosted}");
        // Every kind node shares the host kernel, so a second node's installer would find the
        // first one's profile loaded without its source and refuse.
        let local = kind_config(true);
        assert_eq!(LOCAL_REPLICAS, 1);
        assert_eq!(
            local.matches("role: worker").count(),
            LOCAL_REPLICAS,
            "{local}"
        );
        assert_eq!(
            local.matches("extraMounts").count(),
            LOCAL_REPLICAS,
            "{local}"
        );
        assert_eq!(
            local.matches("hostPath: /sys/kernel/security").count(),
            LOCAL_REPLICAS,
            "{local}"
        );
        let control_plane = local
            .split("  - role: ")
            .find(|node| node.starts_with("control-plane"))
            .expect("a control plane");
        assert!(!control_plane.contains("extraMounts"), "{local}");
    }

    /// The chart's own grammar for `execution.appArmorProfile`, from its closed schema.
    fn chart_profile_pattern() -> fancy_regex::Regex {
        let schema: Value =
            serde_json::from_str(&repository_file("deploy/helm/substrate/values.schema.json"))
                .expect("values.schema.json");
        let pattern = schema["properties"]["execution"]["properties"]["appArmorProfile"]["pattern"]
            .as_str()
            .expect("the chart's appArmorProfile pattern");
        fancy_regex::Regex::new(pattern).expect("the pattern compiles")
    }

    #[test]
    fn every_run_names_a_profile_of_its_own_within_the_charts_grammar() {
        let pattern = chart_profile_pattern();
        let one = lane_profile();
        let other = lane_profile();
        assert_ne!(one, other, "two runs share a profile name");
        for name in [&one, &other] {
            let suffix = name
                .strip_prefix(&format!("{APPARMOR_PROFILE}-"))
                .expect("the default name extended by a suffix");
            assert!(suffix.starts_with("lane-"), "{name}");
            assert!((1..=40).contains(&suffix.len()), "{name}");
            assert!(
                suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
                "{name}"
            );
            assert!(pattern.is_match(name).expect("matches"), "{name}");
            assert_ne!(name.as_str(), APPARMOR_PROFILE);
        }
        // The pattern is the chart's grammar, not a pattern that admits anything.
        for refused in [
            "unconfined",
            "substrate-host-exec-v3",
            "substrate-host-exec-v1-",
            "substrate-host-exec-v1-LANE",
        ] {
            assert!(!pattern.is_match(refused).expect("matches"), "{refused}");
        }
    }

    #[test]
    fn only_the_runs_own_name_loaded_by_someone_else_refuses_the_run() {
        let own = "substrate-host-exec-v1-lane-01k8xyz";
        // Another workload's default profile, a sibling run's lane profile and an unrelated one
        // are not the lane's and decide nothing.
        let others = "docker-default (enforce)\nsubstrate-host-exec-v1 (enforce)\nsubstrate-host-exec-v2 (enforce)\n\
                      substrate-host-exec-v1-lane-01k8abc (enforce)\n";
        assert_eq!(own_profile_verdict(others, own, "worker"), Ok(()));
        assert_eq!(own_profile_verdict("", own, "worker"), Ok(()));
        for mode in ["enforce", "complain"] {
            let listing = format!("{others}{own} ({mode})\n");
            let error = own_profile_verdict(&listing, own, "worker").unwrap_err();
            assert!(error.contains(own), "{error}");
            assert!(error.contains("neither selects nor removes it"), "{error}");
            assert!(!error.contains("substrate-host-exec-v1 "), "{error}");
        }
    }

    #[test]
    fn local_values_enable_execution_and_the_node_profiles_in_the_privileged_namespace() {
        let own = "substrate-host-exec-v1-lane-01k8xyz";
        let values = execution_values(&json!({"execution": {"enabled": false}}), own);
        assert_eq!(values["execution"]["enabled"], true);
        assert_eq!(values["execution"]["appArmorProfile"], own);
        assert_eq!(values["nodeProfiles"]["enabled"], true);
        // The chart refuses a node-profile installer in the release's own namespace.
        assert_eq!(values["nodeProfiles"]["namespace"], NODE_PROFILES_NAMESPACE);
        assert_ne!(NODE_PROFILES_NAMESPACE, PRIVILEGED_NAMESPACE);
        // One worker holds one pod: the chart's anti-affinity places no second.
        assert_eq!(values["replicas"], LOCAL_REPLICAS);
    }

    fn repository_file(path: &str) -> String {
        fs::read_to_string(crate::repo::root().expect("root").join(path)).expect(path)
    }

    #[test]
    fn the_chart_lab_builds_on_the_hosted_lanes_exact_node_image() {
        let recipe = repository_file(LAB_RECIPE);
        let workflow = repository_file(".github/workflows/chart-lane.yml");
        let hosted = workflow
            .lines()
            .find_map(|line| line.trim().strip_prefix("KIND_NODE_IMAGE: "))
            .expect("chart-lane.yml pins KIND_NODE_IMAGE");
        assert!(hosted.contains("@sha256:"), "{hosted}");
        assert_eq!(lab_base(&recipe), Some(hosted));
        assert_eq!(lab_base_verdict(&recipe, hosted), Ok(()));
        let other = "kindest/node:v1.36.0@sha256:0000";
        let error = lab_base_verdict(&recipe, other).unwrap_err();
        assert!(error.contains(other) && error.contains(hosted), "{error}");
        assert!(lab_base_verdict("RUN true\n", hosted).is_err());
    }

    #[test]
    fn the_chart_lab_pins_the_parser_from_the_nodes_own_distribution_and_never_starts_it() {
        let recipe = repository_file(LAB_RECIPE);
        // The node's own Debian snapshot, the exact parser version, and the boot unit masked so
        // the node never loads its distribution's profiles into the kernel it shares.
        assert!(
            recipe.contains("http://snapshot.debian.org/archive/debian/20260803T000000Z"),
            "{recipe}"
        );
        assert!(recipe.contains("apparmor=4.1.0-1"), "{recipe}");
        assert!(recipe.contains("--no-install-recommends"), "{recipe}");
        assert!(
            recipe.contains("systemctl mask apparmor.service"),
            "{recipe}"
        );
        assert!(recipe.contains("test -x /sbin/apparmor_parser"), "{recipe}");
        assert!(
            recipe
                .lines()
                .filter(|line| line.trim_start().starts_with('#'))
                .all(|line| line.starts_with("# syntax=")),
            "{recipe}"
        );
    }

    #[test]
    fn every_checkout_owns_its_chart_lab_tag() {
        let one = lab_image(Path::new("/a/substrate"));
        let other = lab_image(Path::new("/b/substrate"));
        assert!(one.starts_with(&format!("{LAB_REPOSITORY}:")), "{one}");
        assert_ne!(one, other);
        assert_eq!(one, lab_image(Path::new("/a/substrate")));
    }

    #[test]
    fn the_lane_removes_its_own_profile_through_the_source_the_installer_wrote() {
        let values = repository_file("deploy/helm/substrate/values.yaml");
        let kubelet_root = values
            .lines()
            .find_map(|line| line.trim().strip_prefix("kubeletRoot: "))
            .expect("kubeletRoot");
        let template = repository_file("deploy/helm/substrate/templates/node-profiles.yaml");
        assert!(
            template
                .contains("printf \"%s/seccomp/b10x-substrate\" .Values.nodeProfiles.kubeletRoot"),
            "the installer's host directory moved"
        );
        // The installer's source file is `<name>.apparmor` (design 33 § 4).
        let installer = repository_file("crates/substrate-daemon/src/container_profiles.rs");
        assert!(
            installer.contains("format!(\"{name}.apparmor\")"),
            "the installer's AppArmor file name moved"
        );
        let own = lane_profile();
        let argv = removal_argv(&own);
        assert_eq!(
            argv,
            [
                "/sbin/apparmor_parser".to_owned(),
                "--remove".to_owned(),
                format!("{kubelet_root}/seccomp/b10x-substrate/{own}.apparmor"),
            ]
        );
        // The removal names the run's own file and no other profile, the default included.
        assert!(
            !argv[2].ends_with(&format!("/{APPARMOR_PROFILE}.apparmor")),
            "{argv:?}"
        );
    }

    #[test]
    fn the_node_profile_daemonset_is_found_by_the_name_the_chart_gives_it() {
        let template = repository_file("deploy/helm/substrate/templates/node-profiles.yaml");
        assert!(
            template.contains(
                "{{- $name := printf \"%s-node-profiles\" (include \"substrate.name\" .) -}}"
            ),
            "the node-profile DaemonSet's name moved"
        );
        let helpers = repository_file("deploy/helm/substrate/templates/_helpers.tpl");
        assert!(
            helpers.contains("{{- .Release.Name -}}"),
            "substrate.name is no longer the release name"
        );
        assert_eq!(
            node_profiles_daemonset(),
            format!("{RELEASE}-node-profiles")
        );
    }

    #[test]
    fn the_runs_own_name_already_loaded_is_refused_naming_every_scenario() {
        let own = "substrate-host-exec-v1-lane-01k8xyz";
        let message = refusal(&loaded_refusal("substrate-chart-lane-worker", own));
        assert!(message.contains(own), "{message}");
        assert!(!message.contains(".remove"), "{message}");
        for name in SCENARIOS {
            assert!(message.contains(name), "{name}: {message}");
        }
    }

    #[test]
    fn the_undeclared_attempt_counts_only_when_it_reached_the_dial_stage() {
        let answer = |status: u16,
                      payload: Value|
         -> Result<std::result::Result<(u16, Value), rustls::Error>> {
            Ok(Ok((status, payload)))
        };
        for code in DIAL_STAGE {
            let direct = answer(502, json!({"error": {"code": code}}));
            assert!(dial_stage(&direct).is_ok(), "{code}");
            let operation = answer(
                200,
                json!({"op": "x", "outcome": {"status": "failed", "error": {"code": code}}}),
            );
            assert!(dial_stage(&operation).is_ok(), "{code} in an operation");
        }
        for early in [
            answer(
                422,
                json!({"error": {"code": "workspace.git-source-invalid"}}),
            ),
            answer(
                501,
                json!({"error": {"code": "workspace.git-source-unserved"}}),
            ),
            answer(
                403,
                json!({"error": {"code": "workspace.git-locator-refused"}}),
            ),
            answer(201, json!({"result": {"id": "ws_1"}})),
            answer(500, Value::Null),
        ] {
            assert!(dial_stage(&early).is_err(), "{early:?}");
        }
        let timed_out: Result<std::result::Result<(u16, Value), rustls::Error>> =
            Err(std::io::Error::from(ErrorKind::WouldBlock).into());
        assert!(dial_stage(&timed_out).is_ok());
        let refused: Result<std::result::Result<(u16, Value), rustls::Error>> =
            Err(std::io::Error::from(ErrorKind::ConnectionRefused).into());
        assert!(dial_stage(&refused).is_err());
    }

    #[test]
    fn a_create_refused_for_a_name_already_taken_deletes_nothing() {
        // A second run started while the first is still creating its cluster finds the name free,
        // then `kind create` refuses it because the first run holds it. The guard built before the
        // create deletes by that name on drop, which would delete the first run's cluster.
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("kind.log");
        let kind = stand_in(
            directory.path(),
            "kind",
            &format!(
                "echo \"$*\" >> '{}'\nif [ \"$1\" = create ]; then\necho 'ERROR: node(s) already exist for a cluster with the name \"substrate-chart-lane\"' >&2\nexit 1\nfi\nexit 0\n",
                log.display()
            ),
        );
        let args = Args {
            image: "b10x-substrate-daemon:9.9.9".to_owned(),
            node_image: "kindest/node:v1.37.0".to_owned(),
            kind,
            kubectl: PathBuf::from("kubectl-not-used"),
            helm: PathBuf::from("helm-not-used"),
            docker: PathBuf::from("docker-not-used"),
            cluster: "substrate-chart-lane".to_owned(),
            node_profiles: false,
            conformance_runner: None,
            conformance_out: None,
        };
        assert!(
            run(&args).is_err(),
            "kind refused the create, so the run must fail"
        );
        let calls = fs::read_to_string(&log).expect("the stand-in kind was called");
        assert!(
            calls.lines().all(|call| !call.starts_with("delete ")),
            "a refused create deleted a cluster by its name: {calls}"
        );
    }

    #[test]
    fn a_recipe_with_a_second_stage_in_any_spelling_of_from_is_refused() {
        // Docker reads the instruction word without regard to case and after leading white space
        // or a tab, so the image a recipe builds is its last stage, whatever its first `FROM` says.
        let node = "kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5";
        for second in [
            "from alpine:3.20",
            "  FROM alpine:3.20",
            "FROM\talpine:3.20",
        ] {
            let recipe = format!("FROM {node}\nRUN true\n{second}\n");
            assert!(
                lab_base_verdict(&recipe, node).is_err(),
                "a recipe whose last stage is `{second}` passes as built on {node}"
            );
        }
    }

    /// An executable shell script at `directory/name` with `body` after the interpreter line.
    ///
    /// A child process writes it, so this process never holds it open for writing. A child that
    /// another test forks inherits every descriptor this process holds until it execs, and an exec
    /// of a file any process holds open for writing fails `ETXTBSY`; a rename into place does not
    /// help, because the inode is the same.
    fn stand_in(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = directory.join(name);
        // The body goes in on a pipe, not as an argument, so its size is not bounded by
        // MAX_ARG_STRLEN; only the child ever holds the script open for writing.
        let mut writer = Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\"", "stand-in"])
            .arg(&path)
            .stdin(Stdio::piped())
            .spawn()
            .expect("the stand-in writer");
        {
            use std::io::Write as _;
            let mut input = writer.stdin.take().expect("the writer's stdin");
            input
                .write_all(format!("#!/bin/sh\n{body}").as_bytes())
                .expect("the stand-in body");
        }
        let status = writer.wait().expect("the stand-in writer");
        assert!(
            status.success(),
            "writing {} exited {status}",
            path.display()
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    }

    fn stand_in_args(kind: PathBuf) -> Args {
        Args {
            image: "b10x-substrate-daemon:9.9.9".to_owned(),
            node_image: "kindest/node:v1.37.0".to_owned(),
            kind,
            kubectl: PathBuf::from("kubectl-not-used"),
            helm: PathBuf::from("helm-not-used"),
            docker: PathBuf::from("docker-not-used"),
            cluster: "substrate-chart-lane".to_owned(),
            node_profiles: false,
            conformance_runner: None,
            conformance_out: None,
        }
    }

    #[test]
    fn a_source_with_a_carriage_return_declares_no_scenarios() {
        // `release.yml` counts the array with awk, which keeps a carriage return in each line and
        // so matches none of a CRLF source; `lines()` strips it and would count four.
        let source = include_str!("chart_cluster.rs");
        assert_eq!(declared_scenarios(source), Some(SCENARIOS.len()));
        assert_eq!(declared_scenarios(&source.replace('\n', "\r\n")), None);
        let one = source.replacen(
            "    \"execution bootstrap\",\n",
            "    \"execution bootstrap\",\r\n",
            1,
        );
        assert_ne!(one, source);
        assert_eq!(declared_scenarios(&one), None);
    }

    #[test]
    fn a_cluster_name_already_taken_is_refused_naming_its_removal_command() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("kind.log");
        let kind = stand_in(
            directory.path(),
            "kind",
            &format!(
                "echo \"$*\" >> '{}'\nif [ \"$1\" = get ]; then echo substrate-chart-lane; fi\nexit 0\n",
                log.display()
            ),
        );
        let error = run(&stand_in_args(kind)).expect_err("the name is taken");
        let message = format!("{error:#}");
        assert!(
            message.contains("`kind delete cluster --name substrate-chart-lane`"),
            "{message}"
        );
        let calls = fs::read_to_string(&log).expect("kind was asked");
        assert_eq!(
            calls, "get clusters\n",
            "the refusal ran more than the listing"
        );
    }

    #[test]
    fn an_interrupted_command_is_stopped_and_refused() {
        let stop = Arc::new(AtomicBool::new(false));
        let setter = Arc::clone(&stop);
        let signal = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            setter.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = output(Command::new("sleep").arg("30"), &stop);
        signal.join().expect("the setter");
        let error = result.expect_err("an interrupted command is no success");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the command ran on for {:?} after the signal",
            started.elapsed()
        );
        assert!(format!("{error:#}").contains("interrupted"), "{error:#}");
        // Once interrupted, the next command is not started at all.
        let directory = tempfile::tempdir().expect("temporary directory");
        let marker = directory.path().join("started");
        let touch = output(Command::new("touch").arg(&marker), &stop);
        assert!(touch.is_err());
        assert!(!marker.exists(), "a command started after the signal");
    }

    #[test]
    fn a_signal_during_the_run_deletes_the_cluster_it_created_and_fails() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = directory.path().join("kind.log");
        let kind = stand_in(
            directory.path(),
            "kind",
            &format!(
                "echo \"$*\" >> '{}'\nif [ \"$1\" = load ]; then exec sleep 30; fi\nexit 0\n",
                log.display()
            ),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let setter = Arc::clone(&stop);
        let signal = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            setter.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = run_until(&stand_in_args(kind), &stop);
        signal.join().expect("the setter");
        assert!(
            !matches!(result, Ok(code) if code == ExitCode::SUCCESS),
            "an interrupted run succeeded"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the run went on for {:?} after the signal",
            started.elapsed()
        );
        let calls = fs::read_to_string(&log).expect("kind was asked");
        assert!(
            calls
                .lines()
                .any(|call| call.starts_with("delete cluster --name substrate-chart-lane ")),
            "the interrupted run left its cluster: {calls}"
        );
    }

    #[test]
    fn a_failed_helm_uninstall_is_a_removal_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let helm = stand_in(
            directory.path(),
            "helm",
            "if [ \"$1\" = uninstall ]; then echo 'Error: uninstall: timed out' >&2; exit 1; fi\nexit 0\n",
        );
        let kubectl = stand_in(directory.path(), "kubectl", "exit 0\n");
        let docker = stand_in(
            directory.path(),
            "docker",
            "echo 'substrate-host-exec-v2 (enforce)'\nexit 0\n",
        );
        let mut args = stand_in_args(PathBuf::from("kind-not-used"));
        args.helm = helm;
        args.kubectl = kubectl;
        args.docker = docker;
        // Cleanup runs after a signal, so the lane's stop flag is set and the removal runs anyway.
        let stopped = AtomicBool::new(true);
        let lane = Lane {
            args: &args,
            kubeconfig: directory.path().join("kubeconfig"),
            work: directory.path().to_path_buf(),
            profile: "substrate-host-exec-v2-lane-01k8xyz".to_owned(),
            seccomp: None,
            stop: &stopped,
        };
        let error = remove_own_profile(&lane, "substrate-chart-lane-worker")
            .expect_err("a failed uninstall is a failed removal");
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    }

    #[test]
    fn a_stand_in_starts_while_other_threads_fork() {
        // Another test's spawn forks this process; a child forked while a stand-in is open for
        // writing holds that descriptor until it execs, and an exec of the stand-in in that window
        // fails `ETXTBSY` (main's Gate run 41234221370).
        const STARTS: usize = 1000;
        let directory = tempfile::tempdir().expect("temporary directory");
        let forking = Arc::new(AtomicBool::new(true));
        let forkers: Vec<_> = (0..4)
            .map(|_| {
                let forking = Arc::clone(&forking);
                thread::spawn(move || {
                    while forking.load(Ordering::SeqCst) {
                        let _ = Command::new("true")
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                })
            })
            .collect();
        let mut failed = Vec::new();
        for start in 0..STARTS {
            let script = stand_in(directory.path(), &format!("stand-in-{start}"), "exit 0\n");
            // The spawn is where `ETXTBSY` surfaces; `output` would add its 50 ms poll per start.
            let started = Command::new(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match started {
                Ok(status) if status.success() => {}
                Ok(status) => failed.push(format!("{} exited {status}", script.display())),
                Err(error) => failed.push(format!("starting {}: {error}", script.display())),
            }
        }
        forking.store(false, Ordering::SeqCst);
        for forker in forkers {
            forker.join().expect("the forker");
        }
        assert!(
            failed.is_empty(),
            "{} of {STARTS} stand-ins failed to start; the first: {}",
            failed.len(),
            failed[0]
        );
    }

    #[test]
    fn an_all_passed_run_that_is_no_record_says_so_last_on_stderr() {
        let at = UNIX_EPOCH + Duration::from_hours(497_986);
        let report = report_lines(
            Decision::NoRecord("HEAD moved from a to b during the run".to_owned()),
            "a",
            at,
        );
        assert!(
            report.success,
            "an all-passed run exits 0, as quota-fixture does"
        );
        assert_eq!(
            report.stderr.last().map(String::as_str),
            Some("chart lane cluster: not a release record")
        );
        assert!(
            report
                .stdout
                .iter()
                .all(|line| !line.starts_with("Chart-lane:"))
        );
        let record = report_lines(Decision::Record(4), "a", at);
        assert!(record.success && record.stderr.is_empty());
        assert_eq!(
            record.stdout.last().map(String::as_str),
            Some("Chart-lane: a 4 2026-10-23T10:00:00Z")
        );
        let failed = report_lines(Decision::Failed("x".to_owned()), "a", at);
        assert!(!failed.success && failed.stdout.is_empty());
    }

    #[test]
    fn the_lane_reads_the_chart_and_seccomp_profile_from_the_recorded_commit() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let root = repository.path();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(root)
                .args([
                    "-c",
                    "user.name=lane",
                    "-c",
                    "user.email=lane@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        fs::create_dir_all(root.join(CHART).join("templates")).expect("chart directory");
        fs::create_dir_all(root.join("deploy/seccomp")).expect("seccomp directory");
        fs::write(root.join(CHART).join("Chart.yaml"), "committed\n").expect("chart");
        fs::write(root.join(CHART).join("templates/a.yaml"), "committed\n").expect("template");
        fs::write(root.join(SECCOMP_SOURCE), "{\"committed\": true}\n\n").expect("seccomp");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "committed"]);
        let sha = git_text(root, &["rev-parse", "HEAD"]).expect("HEAD");
        fs::write(root.join(CHART).join("Chart.yaml"), "edited\n").expect("edit");
        fs::write(root.join(CHART).join("templates/b.yaml"), "untracked\n").expect("add");
        fs::write(root.join(SECCOMP_SOURCE), "edited\n").expect("edit");
        let into = tempfile::tempdir().expect("extraction directory");
        let (chart_dir, seccomp) =
            committed_sources(root, &sha, into.path()).expect("the committed sources");
        assert!(
            chart_dir.starts_with(into.path()),
            "{}",
            chart_dir.display()
        );
        assert_eq!(
            fs::read_to_string(chart_dir.join("Chart.yaml")).expect("chart"),
            "committed\n"
        );
        assert!(!chart_dir.join("templates/b.yaml").exists());
        assert_eq!(seccomp, "{\"committed\": true}\n\n");
    }

    /// Adversary pass, wave D342 unit U2: `stand_in` writes through a `/bin/sh` child, and the
    /// script it leaves must be the bytes `fs::write` left before.
    mod adversary_d342 {
        use super::*;
        use std::os::unix::fs::PermissionsExt as _;

        fn expected(body: &str) -> Vec<u8> {
            format!("#!/bin/sh\n{body}").into_bytes()
        }

        #[test]
        fn a_stand_in_holds_its_body_byte_for_byte() {
            let directory = tempfile::tempdir().expect("temporary directory");
            let bodies = [
                "",
                "exit 0\n",
                "%s %d %% %b\n",
                "-n\n",
                "--\n",
                "-e x",
                "\\n\\t\\\\ \\0 \\c tail\n",
                "'\"$1 $@ `x` $(y)\n",
                "no trailing newline",
                "\n\n\n",
                "a\r\nb\r\n",
                "\u{e9} \u{2713}\n",
                "\t leading tab and trailing spaces   \n",
            ];
            for (index, body) in bodies.iter().enumerate() {
                let path = stand_in(directory.path(), &format!("s{index}"), body);
                assert_eq!(
                    fs::read(&path).expect("the stand-in"),
                    expected(body),
                    "body {body:?}"
                );
                let mode = fs::metadata(&path).expect("metadata").permissions().mode();
                assert_eq!(mode & 0o777, 0o755, "body {body:?}");
            }
        }

        #[test]
        fn a_stand_in_written_over_an_existing_longer_file_is_truncated() {
            let directory = tempfile::tempdir().expect("temporary directory");
            stand_in(directory.path(), "kind", &"echo long\n".repeat(50));
            let path = stand_in(directory.path(), "kind", "exit 3\n");
            assert_eq!(fs::read(&path).expect("the stand-in"), expected("exit 3\n"));
            let status = Command::new(&path)
                .stdin(Stdio::null())
                .status()
                .expect("start");
            assert_eq!(status.code(), Some(3));
        }

        #[test]
        #[should_panic(expected = "exited")]
        fn a_stand_in_that_cannot_be_written_is_reported() {
            let directory = tempfile::tempdir().expect("temporary directory");
            stand_in(&directory.path().join("missing"), "kind", "exit 0\n");
        }

        /// `fs::write` took any `&str`; one argv string is capped at 128 KiB on Linux
        /// (`MAX_ARG_STRLEN`), so a larger body is no longer written.
        #[test]
        fn a_stand_in_body_larger_than_one_argument_is_written() {
            let directory = tempfile::tempdir().expect("temporary directory");
            let body = format!("# {}\nexit 0\n", "x".repeat(200 * 1024));
            let path = stand_in(directory.path(), "large", &body);
            assert_eq!(fs::read(&path).expect("the stand-in"), expected(&body));
        }
    }
}
