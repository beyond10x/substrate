//! `cargo xtask namespace-lane check|observe`: the skeleton of design 34's kind lane (§ 10.2) and
//! its first observations (§ 10.3).
//!
//! `check` renders the lane's objects from fixed inputs and holds them to the committed goldens
//! under `xtask/tests/golden/namespace-lane/`; it asserts that the Role the lane installs is ADR
//! 0047 decision 3's table as the ADR itself writes it, that no driver-side object is of a
//! cluster-scoped kind, and that every driver-side object carries the ownership keys of design 34
//! § 4. It runs in seconds and needs no cluster (`.github/workflows/namespace-lane.yml`).
//!
//! `observe` runs O1 to O4, O6 to O8 and O10 on a kind cluster of its own, on a workstation. It
//! uses `b10x-substrate-kubernetes-client`, never `kubectl`, and reads only the kubeconfig it
//! creates or is given: never `KUBECONFIG` and never `~/.kube`. Every command it starts gets
//! `KUBECONFIG` set to the run's own file, and it refuses a kubeconfig whose server is not
//! `https://127.0.0.1:<port>`, kind's loopback endpoint, or that lies under `~/.kube`. The cluster
//! is named `substrate-namespace-lane-<id>`, fresh per run; `kind create` and `kind delete` always
//! name it and the run's kubeconfig. The deletion is armed only once `kind create` succeeded, runs
//! on every exit path and on SIGINT or SIGTERM, and the commands that remove the cluster and the
//! run's local registry by hand are printed when each is created. Given `--admin-kubeconfig`, the
//! verb creates no cluster and deletes only the two namespaces it created.
//!
//! It prints `Namespace-lane: <sha> <observation-count> <RFC 3339 UTC>` only when every declared
//! observation ran (held or not), `HEAD` did not move and the checkout was clean before and after.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::chart_cluster::{INTERRUPTED, output};
use crate::lane_record::{git_text, record_line};
use crate::namespace_lane_observe::{self as observe, Outcome};
use crate::namespace_lane_render::{self as render, CLUSTER_SCOPED_ADMIN_KINDS, DRIVER_KINDS};
use crate::report::Report;

/// The goldens `check` compares against, relative to the workspace root.
pub const GOLDENS: &str = "xtask/tests/golden/namespace-lane";
/// ADR 0047, whose decision 3 table the Role must equal.
pub const ADR: &str =
    "adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md";
/// The prefix of every cluster this lane creates; nothing else is ever deleted.
pub const CLUSTER_PREFIX: &str = "substrate-namespace-lane-";
/// The kind node image the pod and chart lanes pin.
pub const KIND_NODE_IMAGE: &str =
    "kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5";
/// The local registry of design 34 § 10.2 (D9), by digest.
pub const REGISTRY_IMAGE: &str = "docker.io/library/registry:2@sha256:a3d8aaa63ed8681a604f1dea0aa03f100d5895b6a58ace528858a7b332415373";
/// busybox 1.37.0 by its image index digest, as the pod lane pins it.
pub const BUSYBOX_REPOSITORY: &str = "docker.io/library/busybox";
pub const BUSYBOX_DIGEST: &str =
    "sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
const TRAILER: &str = "Namespace-lane";

/// The observations this unit runs (design 34 § 12 U5); `observe` records only when each ran.
pub const OBSERVATIONS: [&str; 8] = ["O1", "O2", "O3", "O4", "O6", "O7", "O8", "O10"];

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Render the lane's objects against the goldens and check the Role and ownership keys.
    Check {
        /// Write the goldens from the current rendering instead of comparing.
        #[arg(long)]
        bless: bool,
    },
    /// Run the observations on a kind cluster of the run's own and print the record line.
    Observe(ObserveArgs),
}

#[derive(Debug, ClapArgs)]
pub struct ObserveArgs {
    /// The `kind` executable.
    #[arg(long, default_value = "kind")]
    kind: PathBuf,
    /// The `docker` executable.
    #[arg(long, default_value = "docker")]
    docker: PathBuf,
    /// The pinned kind node image.
    #[arg(long, default_value = KIND_NODE_IMAGE)]
    node_image: String,
    /// The JSON admin kubeconfig of an existing kind cluster: no cluster and no registry is
    /// created, and only the run's two namespaces are deleted. Its server must be
    /// `https://127.0.0.1:<port>`, and it may not lie under `~/.kube`.
    #[arg(long)]
    admin_kubeconfig: Option<PathBuf>,
}

/// `cargo xtask namespace-lane <verb>`.
pub fn run(args: &Args) -> Result<ExitCode> {
    match &args.verb {
        Verb::Check { bless } => {
            let root = crate::repo::root()?;
            if *bless {
                bless_goldens(&root)?;
                return Ok(ExitCode::SUCCESS);
            }
            Ok(check(&root)?.emit())
        }
        Verb::Observe(args) => {
            crate::chart_cluster::arm_signals()?;
            let result = run_observe(args, &INTERRUPTED);
            if INTERRUPTED.load(Ordering::SeqCst) {
                eprintln!(
                    "namespace lane: interrupted by a signal; the run removed what it created and \
                     is no record"
                );
                return result.and(Ok(ExitCode::FAILURE));
            }
            result
        }
    }
}

fn bless_goldens(root: &Path) -> Result<()> {
    let directory = root.join(GOLDENS);
    fs::create_dir_all(&directory)?;
    for (stem, object, _) in render::goldens() {
        fs::write(
            directory.join(format!("{stem}.json")),
            render::golden_text(&object),
        )?;
    }
    Ok(())
}

/// The static half of the lane.
///
/// # Errors
///
/// The ADR or the golden directory cannot be read.
pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    let rendered = render::goldens();
    let directory = root.join(GOLDENS);
    let mut expected: Vec<String> = Vec::new();
    for (stem, object, _) in &rendered {
        let file = format!("{stem}.json");
        match fs::read_to_string(directory.join(&file)) {
            Ok(text) if text == render::golden_text(object) => {}
            Ok(_) => failures.push(format!(
                "{GOLDENS}/{file}: the lane renders a different object; review the change and \
                 rewrite it with `cargo xtask namespace-lane check --bless`"
            )),
            Err(error) => failures.push(format!("{GOLDENS}/{file}: {error}")),
        }
        expected.push(file);
    }
    let mut present: Vec<String> = fs::read_dir(&directory)
        .with_context(|| format!("reading {GOLDENS}"))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    present.sort();
    for file in present {
        if !expected.contains(&file) {
            failures.push(format!(
                "{GOLDENS}/{file}: no rendered object has this golden"
            ));
        }
    }
    let adr = fs::read_to_string(root.join(ADR)).with_context(|| format!("reading {ADR}"))?;
    failures.extend(role_failures(&adr));
    failures.extend(object_failures(&rendered));
    if failures.is_empty() {
        Ok(Report::passed(format!(
            "namespace lane check: {} rendered objects equal their goldens; the Role is ADR 0047 \
             decision 3's {} rows; every driver-side object is namespaced and carries the {} \
             ownership keys",
            rendered.len(),
            render::ROLE_RULES.len(),
            render::OWNERSHIP_KEYS.len()
        )))
    } else {
        Ok(Report::failed(failures))
    }
}

/// Where the Role the lane installs differs from decision 3's table in `adr`.
fn role_failures(adr: &str) -> Vec<String> {
    let table = match render::adr_role_table(adr) {
        Ok(table) => table,
        Err(error) => return vec![format!("{ADR}: {error}")],
    };
    let installed = render::role_grants();
    if installed == table {
        return Vec::new();
    }
    vec![format!(
        "the Role the lane installs is not ADR 0047 decision 3's table: installed {installed:?}, \
         decision 3 {table:?}"
    )]
}

/// The rendered objects' scope and ownership failures.
fn object_failures(rendered: &[(String, Value, bool)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (stem, object, driver) in rendered {
        let kind = object["kind"].as_str().unwrap_or_default();
        let namespace = object["metadata"]["namespace"].as_str();
        if *driver {
            if !DRIVER_KINDS.contains(&kind) {
                failures.push(format!(
                    "{stem}: the driver side renders {kind:?}, not one of the namespaced {DRIVER_KINDS:?}"
                ));
            }
            if namespace.is_none_or(str::is_empty) {
                failures.push(format!("{stem}: a driver-side object names no namespace"));
            }
            for (member, key) in render::OWNERSHIP_KEYS {
                if object["metadata"][member][key]
                    .as_str()
                    .is_none_or(str::is_empty)
                {
                    failures.push(format!("{stem}: metadata.{member} carries no {key}"));
                }
            }
        } else if CLUSTER_SCOPED_ADMIN_KINDS.contains(&kind) {
            if namespace.is_some() {
                failures.push(format!("{stem}: a cluster-scoped {kind} names a namespace"));
            }
        } else if namespace.is_none_or(str::is_empty) {
            failures.push(format!(
                "{stem}: {kind:?} is neither namespaced nor the cluster-scoped kinds \
                 {CLUSTER_SCOPED_ADMIN_KINDS:?}"
            ));
        }
    }
    failures
}

/// The port of `server` when it is exactly `https://127.0.0.1:<port>`, kind's loopback endpoint.
///
/// # Errors
///
/// Why any other server is refused.
pub fn loopback_port(server: &str) -> Result<u16, String> {
    let refused = || {
        format!(
            "the kubeconfig's server {server:?} is not https://127.0.0.1:<port>, a kind cluster's \
             loopback endpoint; the lane refuses every other cluster"
        )
    };
    let port = server
        .strip_prefix("https://127.0.0.1:")
        .ok_or_else(refused)?;
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(refused());
    }
    match port.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(refused()),
    }
}

/// Refuses a kubeconfig path under `home/.kube`, before anything reads it.
///
/// # Errors
///
/// The path lies under `~/.kube`.
pub fn refuse_home_kube(path: &Path, home: Option<&Path>) -> Result<(), String> {
    let Some(home) = home else {
        return Ok(());
    };
    let kube = home.join(".kube");
    let absolute = |path: &Path| {
        fs::canonicalize(path).unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_or_else(|_| path.to_path_buf(), |working| working.join(path))
            }
        })
    };
    let resolved = absolute(path);
    if resolved.starts_with(&kube) || resolved.starts_with(absolute(&kube)) {
        return Err(format!(
            "{} lies under ~/.kube; the lane reads only the kubeconfig it creates or one written \
             for a lane cluster elsewhere",
            path.display()
        ));
    }
    Ok(())
}

/// The server of the current context's cluster in a JSON kubeconfig.
///
/// # Errors
///
/// No current context, cluster or server.
pub fn current_server(config: &Value) -> Result<&str, String> {
    let context = config["current-context"]
        .as_str()
        .ok_or("the kubeconfig names no current context")?;
    let cluster = config["contexts"]
        .as_array()
        .and_then(|contexts| contexts.iter().find(|entry| entry["name"] == context))
        .and_then(|entry| entry["context"]["cluster"].as_str())
        .ok_or("the current context names no cluster")?;
    config["clusters"]
        .as_array()
        .and_then(|clusters| clusters.iter().find(|entry| entry["name"] == cluster))
        .and_then(|entry| entry["cluster"]["server"].as_str())
        .ok_or_else(|| format!("the kubeconfig's cluster {cluster:?} names no server"))
}

/// The JSON admin kubeconfig for `namespace`, from the YAML kubeconfig `kind create cluster
/// --kubeconfig` wrote: its one cluster's server and CA data, its one user's client certificate
/// and key. The server is refused unless it is kind's loopback endpoint.
///
/// # Errors
///
/// The YAML is not kind's form, or its server is refused.
pub fn admin_from_kind(yaml: &str, namespace: &str) -> Result<Value, String> {
    let documents = crate::chart_yaml::documents(yaml).map_err(|error| format!("{error:#}"))?;
    let [config] = documents.as_slice() else {
        return Err("kind's kubeconfig is not one YAML document".into());
    };
    let server = current_server(config)?;
    loopback_port(server)?;
    let cluster = config["clusters"]
        .as_array()
        .and_then(|clusters| clusters.first())
        .map(|entry| &entry["cluster"])
        .ok_or("kind's kubeconfig names no cluster")?;
    let user = config["users"]
        .as_array()
        .and_then(|users| users.first())
        .map(|entry| &entry["user"])
        .ok_or("kind's kubeconfig names no user")?;
    let member = |object: &Value, name: &str| {
        object[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("kind's kubeconfig carries no {name}"))
    };
    Ok(json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "lane",
        "clusters": [{"name": "lane", "cluster": {
            "server": server,
            "certificate-authority-data": member(cluster, "certificate-authority-data")?,
        }}],
        "users": [{"name": "lane-admin", "user": {
            "client-certificate-data": member(user, "client-certificate-data")?,
            "client-key-data": member(user, "client-key-data")?,
        }}],
        "contexts": [{"name": "lane", "context": {
            "cluster": "lane", "user": "lane-admin", "namespace": namespace,
        }}],
    }))
}

/// `admin` with its user replaced by the driver's bearer `token`, for `namespace`.
pub fn driver_config(admin: &Value, token: &str, namespace: &str) -> Value {
    let mut config = admin.clone();
    config["users"] = json!([{"name": "lane-driver", "user": {"token": token}}]);
    config["contexts"] = json!([{"name": "lane", "context": {
        "cluster": "lane", "user": "lane-driver", "namespace": namespace,
    }}]);
    config
}

/// Writes `config` owner-only.
fn write_private(path: &Path, config: &Value) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(&serde_json::to_vec(config)?)?;
    Ok(())
}

/// A client from the JSON kubeconfig at `path`, whose server must be kind's loopback endpoint.
fn client(path: &Path) -> Result<substrate_kubernetes_client::Api> {
    let bytes =
        substrate_kubernetes_client::kubeconfig::read(path).map_err(|error| anyhow!(error))?;
    let config: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("{} is not a JSON kubeconfig", path.display()))?;
    loopback_port(current_server(&config).map_err(|error| anyhow!(error))?)
        .map_err(|error| anyhow!(error))?;
    let (api, ()) = substrate_kubernetes_client::kubeconfig::parse(&bytes, |_, _| Ok(()))
        .map_err(|error| anyhow!(error))?;
    Ok(api)
}

/// A command whose `KUBECONFIG` is the run's own file, whatever the environment holds.
fn tool(program: &Path, kubeconfig: &Path) -> Command {
    let mut command = Command::new(program);
    command.env("KUBECONFIG", kubeconfig);
    command
}

/// A fresh, lowercase id for this run's names.
fn run_id() -> String {
    let mut id = render::nonce(ulid::Ulid::generate().0);
    id.truncate(12);
    id
}

/// The cluster name of run `id`.
pub fn cluster_name(id: &str) -> String {
    format!("{CLUSTER_PREFIX}{id}")
}

/// One control plane and one worker; every kubelet holds `podPidsLimit: 1024` and
/// `containerLogMaxSize: 10Mi` (§ 10.2), and every containerd reads registry hosts from
/// `/etc/containerd/certs.d`, where the run points `localhost:<port>` at its local registry.
pub fn kind_config() -> &'static str {
    concat!(
        "kind: Cluster\n",
        "apiVersion: kind.x-k8s.io/v1alpha4\n",
        "kubeadmConfigPatches:\n",
        "  - |\n",
        "    kind: KubeletConfiguration\n",
        "    podPidsLimit: 1024\n",
        "    containerLogMaxSize: 10Mi\n",
        "containerdConfigPatches:\n",
        "  - |-\n",
        "    [plugins.\"io.containerd.grpc.v1.cri\".registry]\n",
        "      config_path = \"/etc/containerd/certs.d\"\n",
        "nodes:\n",
        "  - role: control-plane\n",
        "  - role: worker\n",
    )
}

/// Deletes the run's cluster on every exit path; built only once `kind create` succeeded.
struct Cluster<'a> {
    kind: &'a Path,
    name: String,
    kubeconfig: PathBuf,
}

impl Drop for Cluster<'_> {
    fn drop(&mut self) {
        use std::os::unix::process::CommandExt as _;
        let deleted = tool(self.kind, &self.kubeconfig)
            .args(["delete", "cluster", "--name", &self.name, "--kubeconfig"])
            .arg(&self.kubeconfig)
            .stdin(Stdio::null())
            .process_group(0)
            .status();
        if !deleted
            .as_ref()
            .is_ok_and(std::process::ExitStatus::success)
        {
            eprintln!(
                "namespace lane: deleting kind cluster {} failed ({deleted:?}); remove it with \
                 `kind delete cluster --name {}`",
                self.name, self.name
            );
        }
    }
}

/// Removes the run's registry container on every exit path; built only once it started.
struct Registry<'a> {
    docker: &'a Path,
    name: String,
    kubeconfig: PathBuf,
}

impl Drop for Registry<'_> {
    fn drop(&mut self) {
        use std::os::unix::process::CommandExt as _;
        let removed = tool(self.docker, &self.kubeconfig)
            .args(["rm", "--force", &self.name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .process_group(0)
            .status();
        if !removed
            .as_ref()
            .is_ok_and(std::process::ExitStatus::success)
        {
            eprintln!(
                "namespace lane: removing the registry container {} failed ({removed:?}); remove \
                 it with `docker rm --force {}`",
                self.name, self.name
            );
        }
    }
}

/// What a finished run prints.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Every observation ran at an unmoved, clean `HEAD`: print the record.
    Record(usize),
    /// Every observation ran, but the run is no record for any commit.
    NoRecord(String),
    /// An observation did not run.
    Failed(String),
}

/// The decision for `outcomes`, one per observation name.
pub fn decision(
    outcomes: &[(String, Result<Outcome, String>)],
    sha: &str,
    head_after: &str,
    clean_before: bool,
    clean_after: bool,
) -> Decision {
    let mut missing = Vec::new();
    for name in OBSERVATIONS {
        match outcomes
            .iter()
            .filter(|(observed, _)| observed == name)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [(_, Ok(_))] => {}
            [(_, Err(error))] => missing.push(format!("{name} did not run: {error}")),
            [] => missing.push(format!("{name} did not run")),
            _ => missing.push(format!("{name} ran more than once")),
        }
    }
    if let Some((name, _)) = outcomes
        .iter()
        .find(|(name, _)| !OBSERVATIONS.contains(&name.as_str()))
    {
        missing.push(format!("{name} is not a declared observation"));
    }
    if !missing.is_empty() {
        return Decision::Failed(missing.join("; "));
    }
    if head_after != sha {
        return Decision::NoRecord(format!(
            "HEAD moved from {sha} to {head_after} during the run"
        ));
    }
    if !(clean_before && clean_after) {
        return Decision::NoRecord(format!(
            "the checkout was not clean {} the run",
            if clean_before { "after" } else { "before" }
        ));
    }
    Decision::Record(OBSERVATIONS.len())
}

fn run_observe(args: &ObserveArgs, stop: &AtomicBool) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let sha = git_text(&root, &["rev-parse", "HEAD"])?;
    let clean_before = git_text(&root, &["status", "--porcelain"])?.is_empty();
    let id = run_id();
    let namespace = format!("snl-{id}");
    let unlabelled = format!("snl-{id}-unlabelled");
    let work = tempfile::tempdir()?;
    let admin_path = work.path().join("admin.json");

    let mut cluster = None;
    let mut registry = None;
    let image;
    let docker_kubeconfig;
    if let Some(given) = &args.admin_kubeconfig {
        given_admin(given, &namespace, &admin_path)?;
        image = format!("{BUSYBOX_REPOSITORY}@{BUSYBOX_DIGEST}");
        docker_kubeconfig = admin_path.clone();
    } else {
        let name = cluster_name(&id);
        let kind_kubeconfig = work.path().join("kind-kubeconfig.yaml");
        cluster = Some(create_cluster(
            args,
            &name,
            &kind_kubeconfig,
            work.path(),
            stop,
        )?);
        let yaml = fs::read_to_string(&kind_kubeconfig)?;
        let admin = admin_from_kind(&yaml, &namespace).map_err(|error| anyhow!(error))?;
        write_private(&admin_path, &admin)?;
        let (started, pushed) = start_registry(args, &name, &kind_kubeconfig, stop)?;
        registry = Some(started);
        image = pushed;
        docker_kubeconfig = kind_kubeconfig;
    }

    let gateway = observe::kind_gateway(&args.docker, &docker_kubeconfig, stop)?;
    let admin = client(&admin_path)?;
    let driver_path = work.path().join("driver.json");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let outcomes = runtime.block_on(async {
        let lane = observe::Lane {
            admin: &admin,
            namespace: &namespace,
            unlabelled: &unlabelled,
            deployment: &format!("lane-{id}"),
            image: &image,
            gateway,
            stop,
        };
        let provisioned = lane
            .provision(|token| {
                let admin_config: Value = serde_json::from_slice(&fs::read(&admin_path)?)?;
                write_private(
                    &driver_path,
                    &driver_config(&admin_config, token, &namespace),
                )?;
                client(&driver_path)
            })
            .await;
        let outcomes = match provisioned {
            Ok(driver) => lane.observe_all(&driver).await,
            Err(error) => OBSERVATIONS
                .iter()
                .map(|name| {
                    (
                        (*name).to_owned(),
                        Err(format!("provisioning failed: {error:#}")),
                    )
                })
                .collect(),
        };
        if cluster.is_none() {
            lane.remove_namespaces().await;
        }
        outcomes
    });
    drop(registry);
    drop(cluster);
    finish(&root, &outcomes, &sha, clean_before)
}

/// The run's summary, one line per observation, then the record line or why there is none.
fn finish(
    root: &Path,
    outcomes: &[(String, Result<Outcome, String>)],
    sha: &str,
    clean_before: bool,
) -> Result<ExitCode> {
    for (name, outcome) in outcomes {
        match outcome {
            Ok(Outcome::Held(evidence)) => println!("namespace lane: {name} held: {evidence}"),
            Ok(Outcome::NotHeld(evidence)) => {
                println!("namespace lane: {name} did not hold: {evidence}");
            }
            Err(error) => println!("namespace lane: {name} did not run: {error}"),
        }
    }
    let head_after = git_text(root, &["rev-parse", "HEAD"])?;
    let clean_after = git_text(root, &["status", "--porcelain"])?.is_empty();
    match decision(outcomes, sha, &head_after, clean_before, clean_after) {
        Decision::Record(count) => {
            println!("{}", record_line(TRAILER, sha, count, SystemTime::now()));
            Ok(ExitCode::SUCCESS)
        }
        Decision::NoRecord(reason) => {
            eprintln!("namespace lane: every observation ran, but no record: {reason}");
            Ok(ExitCode::FAILURE)
        }
        Decision::Failed(reason) => {
            eprintln!("namespace lane: no record: {reason}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Copies the given JSON admin kubeconfig to `admin_path`, its context naming `namespace`; refused
/// under `~/.kube` before it is read, and for any server but kind's loopback endpoint.
fn given_admin(given: &Path, namespace: &str, admin_path: &Path) -> Result<()> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    refuse_home_kube(given, home.as_deref()).map_err(|error| anyhow!(error))?;
    let bytes =
        substrate_kubernetes_client::kubeconfig::read(given).map_err(|error| anyhow!(error))?;
    let mut config: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("{} is not a JSON kubeconfig", given.display()))?;
    loopback_port(current_server(&config).map_err(|error| anyhow!(error))?)
        .map_err(|error| anyhow!(error))?;
    let context = config["current-context"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if let Some(entry) = config["contexts"].as_array_mut().and_then(|contexts| {
        contexts
            .iter_mut()
            .find(|entry| entry["name"] == context.as_str())
    }) {
        entry["context"]["namespace"] = json!(namespace);
    }
    write_private(admin_path, &config)
}

/// Creates the run's cluster `name`, its kubeconfig written to `kubeconfig` and nowhere else; the
/// guard that deletes it exists only once `kind create` succeeded.
fn create_cluster<'a>(
    args: &'a ObserveArgs,
    name: &str,
    kubeconfig: &Path,
    work: &Path,
    stop: &AtomicBool,
) -> Result<Cluster<'a>> {
    let config_file = work.join("kind.yaml");
    fs::write(&config_file, kind_config())?;
    let existing = output(tool(&args.kind, kubeconfig).args(["get", "clusters"]), stop)?;
    if existing.lines().any(|line| line == name) {
        bail!("a kind cluster named {name} already exists; the lane creates its own");
    }
    let created = output(
        tool(&args.kind, kubeconfig)
            .args([
                "create",
                "cluster",
                "--name",
                name,
                "--image",
                &args.node_image,
            ])
            .arg("--config")
            .arg(&config_file)
            .arg("--kubeconfig")
            .arg(kubeconfig)
            .args(["--wait", "180s"]),
        stop,
    );
    if let Err(error) = created {
        // A refused create may be refused for a name another run holds: delete nothing.
        if stop.load(Ordering::SeqCst) {
            eprintln!(
                "namespace lane: kind create was stopped; if it left the cluster {name}, remove \
                 it with `kind delete cluster --name {name}`"
            );
        }
        return Err(error);
    }
    println!(
        "namespace lane: created kind cluster {name}; a run killed before it deletes it leaves \
         it behind, removed with `kind delete cluster --name {name}`"
    );
    Ok(Cluster {
        kind: &args.kind,
        name: name.to_owned(),
        kubeconfig: kubeconfig.to_path_buf(),
    })
}

/// Starts the run's local registry (D9) on the kind network, copies the pinned busybox index into
/// it by `docker buildx imagetools create`, which keeps its digest, and points every node's
/// containerd at it. The image reference the Pods name, by that digest.
fn start_registry<'a>(
    args: &'a ObserveArgs,
    cluster: &str,
    kubeconfig: &Path,
    stop: &AtomicBool,
) -> Result<(Registry<'a>, String)> {
    let name = format!("{cluster}-registry");
    output(
        tool(&args.docker, kubeconfig).args([
            "run",
            "--detach",
            "--name",
            &name,
            "--network",
            "kind",
            "--publish",
            "127.0.0.1::5000",
            REGISTRY_IMAGE,
        ]),
        stop,
    )?;
    let registry = Registry {
        docker: &args.docker,
        name: name.clone(),
        kubeconfig: kubeconfig.to_path_buf(),
    };
    println!(
        "namespace lane: started the local registry {name}; a run killed before it removes it \
         leaves it behind, removed with `docker rm --force {name}`"
    );
    let published = output(
        tool(&args.docker, kubeconfig).args(["port", &name, "5000/tcp"]),
        stop,
    )?;
    let port = published
        .lines()
        .find_map(|line| line.trim().strip_prefix("127.0.0.1:"))
        .ok_or_else(|| anyhow!("the registry published no loopback port: {published:?}"))?
        .to_owned();
    let host = format!("localhost:{port}");
    output(
        tool(&args.docker, kubeconfig).args([
            "buildx",
            "imagetools",
            "create",
            "--tag",
            &format!("{host}/busybox:lane"),
            &format!("{BUSYBOX_REPOSITORY}@{BUSYBOX_DIGEST}"),
        ]),
        stop,
    )?;
    let nodes = output(
        tool(&args.kind, kubeconfig).args(["get", "nodes", "--name", cluster]),
        stop,
    )?;
    for node in nodes.lines().map(str::trim).filter(|node| !node.is_empty()) {
        output(
            tool(&args.docker, kubeconfig).args([
                "exec",
                node,
                "sh",
                "-c",
                "mkdir -p \"$1\" && printf '%s\\n' \"$2\" > \"$1/hosts.toml\"",
                "sh",
                &format!("/etc/containerd/certs.d/{host}"),
                &format!("[host.\"http://{name}:5000\"]"),
            ]),
            stop,
        )?;
    }
    Ok((registry, format!("{host}/busybox@{BUSYBOX_DIGEST}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIND_YAML: &str = "apiVersion: v1\nclusters:\n- cluster:\n    certificate-authority-data: Q0E=\n    server: https://127.0.0.1:41234\n  name: kind-substrate-namespace-lane-x\ncontexts:\n- context:\n    cluster: kind-substrate-namespace-lane-x\n    user: kind-substrate-namespace-lane-x\n  name: kind-substrate-namespace-lane-x\ncurrent-context: kind-substrate-namespace-lane-x\nkind: Config\npreferences: {}\nusers:\n- name: kind-substrate-namespace-lane-x\n  user:\n    client-certificate-data: Q0VSVA==\n    client-key-data: S0VZ\n";

    #[test]
    fn only_kinds_loopback_endpoint_is_accepted() {
        assert_eq!(loopback_port("https://127.0.0.1:41234"), Ok(41234));
        for server in [
            "https://10.0.0.1:6443",
            "https://localhost:6443",
            "https://127.0.0.2:6443",
            "https://127.0.0.1",
            "https://127.0.0.1:",
            "https://127.0.0.1:0",
            "https://127.0.0.1:65536",
            "https://127.0.0.1:6443/",
            "https://127.0.0.1:6443/apis",
            "https://127.0.0.1:+6443",
            "https://127.0.0.1.example.test:6443",
            "http://127.0.0.1:6443",
            "https://[::1]:6443",
            "https://cluster.example.test:443",
        ] {
            let refused = loopback_port(server).expect_err(server);
            assert!(refused.contains(server), "{refused}");
        }
    }

    #[test]
    fn kinds_kubeconfig_becomes_a_json_admin_kubeconfig_for_the_namespace() {
        let admin = admin_from_kind(KIND_YAML, "snl-x").expect("kind's form");
        assert_eq!(current_server(&admin), Ok("https://127.0.0.1:41234"));
        assert_eq!(admin["contexts"][0]["context"]["namespace"], "snl-x");
        assert_eq!(admin["users"][0]["user"]["client-key-data"], "S0VZ");
        let driver = driver_config(&admin, "token-value", "snl-x");
        assert_eq!(driver["users"][0]["user"], json!({"token": "token-value"}));
        assert_eq!(current_server(&driver), Ok("https://127.0.0.1:41234"));
    }

    #[test]
    fn a_kind_kubeconfig_naming_another_server_is_refused() {
        let elsewhere = KIND_YAML.replace("https://127.0.0.1:41234", "https://203.0.113.7:6443");
        let refused = admin_from_kind(&elsewhere, "snl-x").expect_err("non-loopback");
        assert!(refused.contains("https://203.0.113.7:6443"), "{refused}");
    }

    #[test]
    fn a_kubeconfig_under_the_home_kube_directory_is_refused_before_it_is_read() {
        let home = tempfile::tempdir().expect("home");
        let kube = home.path().join(".kube");
        fs::create_dir(&kube).expect(".kube");
        let refused =
            refuse_home_kube(&kube.join("config"), Some(home.path())).expect_err("~/.kube/config");
        assert!(refused.contains("~/.kube"), "{refused}");
        assert!(refuse_home_kube(&home.path().join("lane.json"), Some(home.path())).is_ok());
    }

    #[test]
    fn every_command_the_lane_starts_names_the_runs_own_kubeconfig() {
        let own = Path::new("/run/lane/kubeconfig");
        let command = tool(Path::new("kind"), own);
        let named: Vec<_> = command
            .get_envs()
            .filter(|(key, _)| *key == "KUBECONFIG")
            .collect();
        assert_eq!(
            named,
            [(std::ffi::OsStr::new("KUBECONFIG"), Some(own.as_os_str()))]
        );
    }

    #[test]
    fn cluster_names_are_fresh_lane_names_and_kind_sized() {
        let (first, second) = (run_id(), run_id());
        assert_ne!(first, second);
        let name = cluster_name(&first);
        assert!(name.starts_with(CLUSTER_PREFIX));
        assert!(name.len() + "-control-plane".len() <= 63, "{name}");
        assert!(substrate_kubernetes_client::is_dns_label(&name), "{name}");
        assert_ne!(name, "b10x-dev");
    }

    #[test]
    fn the_static_check_holds_on_the_committed_goldens() {
        let root = crate::repo::root().expect("root");
        let report = check(&root).expect("check");
        assert!(report.failures().is_empty(), "{}", report.failure_text());
    }

    #[test]
    fn a_role_that_differs_from_decision_three_is_refused() {
        let root = crate::repo::root().expect("root");
        let adr = fs::read_to_string(root.join(ADR)).expect("ADR");
        assert!(role_failures(&adr).is_empty());
        let widened = adr.replace("| `pods/log` | `get` |", "| `pods/log` | `get`, `list` |");
        assert_ne!(widened, adr);
        let failures = role_failures(&widened);
        assert_eq!(failures.len(), 1, "{failures:?}");
    }

    #[test]
    fn a_driver_object_without_an_ownership_key_or_of_a_cluster_kind_is_refused() {
        let mut rendered = render::goldens();
        let (_, claim, _) = rendered
            .iter_mut()
            .find(|(stem, _, _)| stem == "driver-claim")
            .expect("claim");
        claim["metadata"]["annotations"] = json!({});
        rendered.push((
            "driver-namespace".to_owned(),
            json!({"kind": "Namespace", "metadata": {"name": "x"}}),
            true,
        ));
        let failures = object_failures(&rendered);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("driver-claim") && failure.contains("operation")),
            "{failures:?}"
        );
        assert!(
            failures.iter().any(
                |failure| failure.contains("driver-namespace") && failure.contains("Namespace")
            ),
            "{failures:?}"
        );
    }

    #[test]
    fn the_record_needs_every_observation_once_at_a_clean_unmoved_head() {
        let all = |outcome: fn(&str) -> Result<Outcome, String>| -> Vec<(String, Result<Outcome, String>)> {
            OBSERVATIONS
                .iter()
                .map(|name| ((*name).to_owned(), outcome(name)))
                .collect()
        };
        let ran = all(|name| {
            if name == "O1" {
                Ok(Outcome::NotHeld("the Pod stayed".to_owned()))
            } else {
                Ok(Outcome::Held("held".to_owned()))
            }
        });
        assert_eq!(decision(&ran, "a", "a", true, true), Decision::Record(8));
        assert!(matches!(
            decision(&ran, "a", "b", true, true),
            Decision::NoRecord(_)
        ));
        assert!(matches!(
            decision(&ran, "a", "a", false, true),
            Decision::NoRecord(_)
        ));
        assert!(matches!(
            decision(&ran, "a", "a", true, false),
            Decision::NoRecord(_)
        ));
        let mut short = ran;
        short.retain(|(name, _)| name != "O10");
        assert!(matches!(
            decision(&short, "a", "a", true, true),
            Decision::Failed(_)
        ));
        let failed = all(|_| Err("no cluster".to_owned()));
        assert!(matches!(
            decision(&failed, "a", "a", true, true),
            Decision::Failed(_)
        ));
    }
}
