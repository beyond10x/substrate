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
//! 3. a workspace, a file in it and an operation replay survive a rolling restart by
//!    `helm upgrade`, and the replayed operation answers the original workspace;
//! 4. the daemon dials a declared Git source and its connection to an undeclared port on the same
//!    host never arrives (the `NetworkPolicy`); if it did not dial the declared one either, the
//!    scenario is reported absent, never passed;
//! 5. the execution-enabled pod is admitted by server dry run in a `privileged` namespace and
//!    refused in a `baseline` one, and the execution-off install itself runs under `restricted`;
//! 6. `helm uninstall` leaves exactly the ledger claims.
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
//! `substrate-host-exec-v1-lane-<ulid>`, fresh and lowercased (design 33 § 7), so it never loads,
//! selects or removes a profile it does not own, the default `substrate-host-exec-v1` included. It
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
//! removed, `HEAD` did not move and the checkout was clean before and after the run.

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
const APPARMOR_PROFILE: &str = "substrate-host-exec-v1";
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
        "the daemon under the substrate-host-exec-v1 Localhost AppArmor profile is not run on kind",
    ),
    (
        "TLS key after the uid drop",
        "reading the 0400 TLS key after the bootstrap drops to uid 65532 is not run on kind",
    ),
];

/// The scenarios the hosted mode runs, in order, on two workers.
const HOSTED: [&str; 6] = [
    "placement",
    "certificate names",
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
/// length is not its entry count.
pub fn declared_scenarios(source: &str) -> Option<usize> {
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

/// The image the chart lab's recipe builds on: its one `FROM` reference.
fn lab_base(recipe: &str) -> Option<&str> {
    let mut bases = recipe
        .lines()
        .filter_map(|line| line.strip_prefix("FROM "))
        .map(str::trim);
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

/// This run's own profile name, `substrate-host-exec-v1-lane-<ulid>` with a fresh lowercased ULID
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

/// Whether the shared kernel's `listing` lets this run load `profile`: refused only when that
/// exact name is already loaded, by something this run did not start. Any other profile, the
/// default `substrate-host-exec-v1` included, is not the lane's and decides nothing.
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
}

impl Lane<'_> {
    fn tool(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env("KUBECONFIG", &self.kubeconfig);
        command
    }

    fn kubectl(&self, args: &[&str]) -> Result<String> {
        output(self.tool(&self.args.kubectl).args(args))
    }

    fn helm(&self, args: &[&str]) -> Result<String> {
        output(self.tool(&self.args.helm).args(args))
    }
}

fn output(command: &mut Command) -> Result<String> {
    let rendered = format!("{command:?}");
    let result = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("starting {rendered}"))?;
    ensure!(
        result.status.success(),
        "{rendered} exited {}: {}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?)
}

/// Deletes the cluster on every exit path.
struct Cluster<'a> {
    args: &'a Args,
    kubeconfig: PathBuf,
}

impl Drop for Cluster<'_> {
    fn drop(&mut self) {
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

#[allow(clippy::too_many_lines)] // The lane in its fixed order: cluster, fixtures, install, six scenarios, teardown.
pub fn run(args: &Args) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let chart_dir = root.join(CHART);
    let (repository, tag) = args
        .image
        .rsplit_once(':')
        .filter(|(repository, _)| !repository.is_empty())
        .ok_or_else(|| anyhow!("--image {:?} is not repository:tag", args.image))?;
    let clusters = output(Command::new(&args.kind).args(["get", "clusters"]))?;
    ensure!(
        !clusters.lines().any(|line| line == args.cluster),
        "a kind cluster named {} already exists; the lane creates and deletes its own",
        args.cluster
    );

    // Local mode reads `HEAD` and the checkout state once, here; a record names this commit, and
    // the image must carry it.
    let start = if args.node_profiles {
        let sha = git_text(&root, &["rev-parse", "HEAD"])?;
        let clean = git_text(&root, &["status", "--porcelain"])?.is_empty();
        let label = output(Command::new(&args.docker).args([
            "image",
            "inspect",
            "--format",
            &format!("{{{{ index .Config.Labels \"{REVISION_LABEL}\" }}}}"),
            &args.image,
        ]))?;
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
    let node_image = match &start {
        Some((_, _, recipe)) => match build_lab(args, &root, recipe, work.path()) {
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
    let cluster = Cluster {
        args,
        kubeconfig: kubeconfig.clone(),
    };
    output(
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
    )?;
    output(Command::new(&args.kind).args([
        "load",
        "docker-image",
        &args.image,
        "--name",
        &args.cluster,
    ]))?;
    let lane = Lane {
        args,
        kubeconfig,
        work: work.path().to_path_buf(),
        profile: if args.node_profiles {
            lane_profile()
        } else {
            APPARMOR_PROFILE.to_owned()
        },
    };
    if args.node_profiles {
        println!(
            "chart lane cluster: this run's own AppArmor profile is {}",
            lane.profile
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
    let gateway = kind_gateway(args)?;

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
                    "removing this run's {profile} through {worker}: {error:#}; it stays loaded \
                     in the host kernel until the host reboots or root removes it by name: sudo \
                     sh -c 'printf %s {profile} > /sys/kernel/security/apparmor/.remove'"
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
    match decision(
        local_verdict(outcomes, failures),
        sha,
        &head_after,
        clean_before,
        clean_after,
    ) {
        Decision::Record(count) => {
            println!(
                "chart lane cluster: all {count} scenarios passed at {sha}; record this line in the annotated tag:"
            );
            println!("{}", evidence_line(sha, count, SystemTime::now()));
            ExitCode::SUCCESS
        }
        Decision::NoRecord(reason) => {
            println!(
                "chart lane cluster: all {} scenarios passed at {sha}, but {reason}; this run is \
                 not a release record for any commit",
                SCENARIOS.len()
            );
            ExitCode::SUCCESS
        }
        Decision::Failed(reason) => {
            eprintln!("chart lane cluster: no record: {reason}");
            ExitCode::FAILURE
        }
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
fn build_lab(args: &Args, root: &Path, recipe: &str, work: &Path) -> Result<String> {
    let image = lab_image(&fs::canonicalize(root)?);
    let context = work.join("chart-lab");
    fs::create_dir(&context)?;
    fs::write(context.join("Dockerfile"), recipe)?;
    let status = Command::new(&args.docker)
        .args(["build", "--tag", &image])
        .arg(&context)
        .stdin(Stdio::null())
        .status()
        .context("docker build")?;
    ensure!(
        status.success(),
        "building {image} from {LAB_RECIPE} failed: {status}"
    );
    Ok(image)
}

/// Removes the `AppArmor` profile this run loaded, through the node holding its source, after
/// nothing on the cluster can load it again: the release is uninstalled and no pod of it remains,
/// so the holding installer cannot restart and reload it. A profile already gone is no error.
fn remove_own_profile(lane: &Lane<'_>, worker: &str) -> Result<()> {
    // An install that never created a release has nothing to uninstall; the pod check decides.
    let _ = lane.helm(&[
        "uninstall",
        RELEASE,
        "-n",
        PRIVILEGED_NAMESPACE,
        "--wait",
        "--timeout",
        "3m",
    ]);
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
    let sandbox = inside(&[
        "/usr/bin/bwrap",
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--die-with-parent",
        "--ro-bind",
        "/",
        "/",
        "/usr/bin/true",
    ]);
    format!(
        "diagnosis of {}: published facts {facts}; daemon cgroup {cgroup:?}; delegated root \
         {root:?} with controllers {controllers:?}; a bubblewrap sandbox in its container: \
         {sandbox:?}",
        pod.name()
    )
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
    let source = fs::read_to_string(crate::repo::root()?.join(SECCOMP_SOURCE))?;
    let profile: Value = serde_json::from_str(&source)?;
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

fn kind_gateway(args: &Args) -> Result<Ipv4Addr> {
    let config = output(Command::new(&args.docker).args([
        "network",
        "inspect",
        "kind",
        "--format",
        "{{json .IPAM.Config}}",
    ]))?;
    let config: Value = serde_json::from_str(config.trim())?;
    config
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| entry["Gateway"].as_str()?.parse::<Ipv4Addr>().ok())
        .ok_or_else(|| anyhow!("the kind network has no IPv4 gateway: {config}"))
}

/// The lane's certificates and the one bearer credential its Identity stub resolves.
struct Material {
    daemon_pem: String,
    daemon_key_pem: String,
    daemon_der: CertificateDer<'static>,
    identity_pem: String,
    identity_der: CertificateDer<'static>,
    identity_key: Vec<u8>,
    pod_names: Vec<String>,
    service_name: String,
    credential: String,
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
        let daemon_key = KeyPair::generate()?;
        let daemon = CertificateParams::new(pod_names.clone())?.self_signed(&daemon_key)?;
        let identity_key = KeyPair::generate()?;
        let identity =
            CertificateParams::new(vec![gateway.to_string()])?.self_signed(&identity_key)?;
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
            service_name: format!("{RELEASE}.{NAMESPACE}.svc.cluster.local"),
            credential,
        })
    }

    fn client(&self) -> Result<Arc<ClientConfig>> {
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

/// A listener on the kind gateway, served from a thread until stopped.
struct Listener {
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

    fn identity(gateway: Ipv4Addr, material: &Material) -> Result<Self> {
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

    fn origin(&self) -> String {
        format!("https://{}", self.address)
    }

    fn stop(mut self) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // the six hosted ones are the hosted lane's, on two workers.
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
        let listing = "docker-default (enforce)\nsubstrate-host-exec-v1 (enforce)\n";
        assert!(profile_loaded(listing, APPARMOR_PROFILE));
        assert!(profile_loaded(
            "substrate-host-exec-v1 (complain)\n",
            APPARMOR_PROFILE
        ));
        assert!(!profile_loaded(
            "docker-default (enforce)\n",
            APPARMOR_PROFILE
        ));
        assert!(!profile_loaded(
            "substrate-host-exec-v10 (enforce)\nx//substrate-host-exec-v1 (enforce)\n",
            APPARMOR_PROFILE
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
            "substrate-host-exec-v2",
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
        let others = "docker-default (enforce)\nsubstrate-host-exec-v1 (enforce)\n\
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
}
