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
//! `https://127.0.0.1:<port>`, kind's loopback endpoint. The cluster is named
//! `substrate-namespace-lane-<id>`, fresh per run; `kind create` and `kind delete` always name it
//! and the run's kubeconfig. The deletion is armed only once `kind create` succeeded, runs on
//! every exit path and on SIGINT or SIGTERM, and the commands that remove the cluster and the
//! run's local registry by hand are printed when each is created.
//!
//! Given `--admin-kubeconfig`, the verb creates no cluster and deletes only the two namespaces it
//! created, and it acts only on a cluster this lane created. Before reading the file it refuses a
//! default kubeconfig: one under `~/.kube` of `HOME` or of the current uid's passwd entry, or a
//! file the `KUBECONFIG` list names. It then refuses a cluster or user member kind never writes
//! (`tls-server-name`, `proxy-url`, `exec`, `auth-provider` among them), and a current context or
//! cluster that is not `kind-substrate-namespace-lane-<id>`; and before any write, a cluster whose
//! API lists no node `substrate-namespace-lane-<id>-control-plane`.
//!
//! After the observations it runs § 10.2 steps 3 to 7 (`xtask/src/namespace_lane_run.rs`): the
//! shipped daemon with `--driver kubernetes`, the vectors' namespace lane, the driver-specific
//! cases of ADR 0047 decision 13 and the cleanup assertions, each case once.
//!
//! It prints `Namespace-lane: <sha> <observation-count> <RFC 3339 UTC>` only when every declared
//! observation ran (held or not), every case of steps 3 to 7 passed or is declared absent with its
//! reason, what the run created was removed, no signal arrived, `HEAD` did not move and the
//! checkout was clean before and after. The count stays the number of observations.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::chart_cluster::{INTERRUPTED, output};
use crate::lane_record::{git_text, record_line};
use crate::namespace_lane_observe::{self as observe, Outcome};
use crate::namespace_lane_render::{self as render, CLUSTER_SCOPED_ADMIN_KINDS, DRIVER_KINDS};
use crate::namespace_lane_run as run;
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
    /// `https://127.0.0.1:<port>`, its context and cluster `kind-substrate-namespace-lane-<id>`
    /// with that cluster's control-plane node, and it may be neither under `~/.kube` nor named by
    /// `KUBECONFIG`.
    #[arg(long)]
    admin_kubeconfig: Option<PathBuf>,
    /// The executor image, by digest, that steps 3 to 7 name; required with `--admin-kubeconfig`,
    /// which starts no registry. Without it the run builds the Dockerfile's `executor` target from
    /// this checkout and pushes it to its own registry.
    #[arg(long)]
    executor_image: Option<String>,
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
            // A record is printed only when no signal had arrived; one that arrives after it
            // changes nothing the run printed.
            if INTERRUPTED.load(Ordering::SeqCst) && !matches!(result, Ok((_, true))) {
                eprintln!(
                    "namespace lane: interrupted by a signal; the run removed what it created and \
                     is no record"
                );
                return result.and(Ok(ExitCode::FAILURE));
            }
            result.map(|(code, _)| code)
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

/// The home directories whose `.kube` the lane refuses: `env_home` (from `HOME`), when set and
/// not empty, and the home directory of the current uid's passwd entry, so the refusal holds
/// whatever `HOME` says.
///
/// # Errors
///
/// Neither is known, so no `~/.kube` could be refused.
pub fn home_directories(
    env_home: Option<PathBuf>,
    passwd_home: Option<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut homes: Vec<PathBuf> = env_home
        .into_iter()
        .chain(passwd_home)
        .filter(|home| !home.as_os_str().is_empty())
        .collect();
    homes.dedup();
    if homes.is_empty() {
        return Err(
            "neither HOME nor the passwd entry of the current uid names a home directory, so the \
             lane cannot refuse ~/.kube; it refuses to run"
                .into(),
        );
    }
    Ok(homes)
}

/// The home directory of the current uid's passwd entry, read with `getpwuid_r`.
pub fn passwd_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;
    // SAFETY: `getuid` takes no argument and cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut size = 4096_usize;
    while size <= 1_048_576 {
        let mut buffer = vec![0 as libc::c_char; size];
        // SAFETY: `passwd` is plain data; all-zero is a valid value that `getpwuid_r` overwrites.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer names a live local of the stated size; the strings `entry` points
        // at live in `buffer`, which outlives every read of them below.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &raw mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &raw mut found,
            )
        };
        if status == libc::ERANGE {
            size *= 4;
            continue;
        }
        if status != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        // SAFETY: `pw_dir` is a NUL-terminated string inside `buffer`, checked non-null above.
        let directory = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
        return Some(PathBuf::from(std::ffi::OsStr::from_bytes(
            directory.to_bytes(),
        )));
    }
    None
}

/// `path` made absolute: canonical when it resolves, else joined to the working directory.
fn absolute(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_or_else(|_| path.to_path_buf(), |working| working.join(path))
        }
    })
}

/// Refuses `path` when it is a default kubeconfig: under `.kube` in any of `homes`, or one of the
/// files the `KUBECONFIG` list `ambient` names, compared by canonical path and by file identity;
/// before anything reads it.
///
/// # Errors
///
/// Why the path is a default kubeconfig.
pub fn refuse_default_kubeconfigs(
    path: &Path,
    homes: &[PathBuf],
    ambient: Option<&std::ffi::OsStr>,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    for home in homes {
        refuse_home_kube(path, Some(home))?;
    }
    let given = absolute(path);
    let identity = |path: &Path| {
        fs::metadata(path)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    };
    let given_identity = identity(path);
    for entry in std::env::split_paths(ambient.unwrap_or_default()) {
        if entry.as_os_str().is_empty() {
            continue;
        }
        let same = absolute(&entry) == given
            || given_identity.is_some_and(|id| identity(&entry) == Some(id));
        if same {
            return Err(format!(
                "{} is the file KUBECONFIG names ({}), a default kubeconfig; the lane reads only \
                 the kubeconfig it creates or one written for a lane cluster elsewhere",
                path.display(),
                entry.display()
            ));
        }
    }
    Ok(())
}

/// The cluster members kind writes into a kubeconfig, and the only ones a given one may carry:
/// any other can point the client at another cluster (`tls-server-name`, `proxy-url`) or change
/// whom it trusts.
pub const GIVEN_CLUSTER_MEMBERS: [&str; 2] = ["server", "certificate-authority-data"];
/// The user members a given kubeconfig may carry: kind's client certificate and key, or a bearer
/// token; any other runs a plugin (`exec`, `auth-provider`) or changes who the client is.
pub const GIVEN_USER_MEMBERS: [&str; 3] = ["client-certificate-data", "client-key-data", "token"];

/// Refuses a given kubeconfig whose current cluster or user carries a member outside
/// [`GIVEN_CLUSTER_MEMBERS`] and [`GIVEN_USER_MEMBERS`], naming it.
///
/// # Errors
///
/// The first such member, or no current cluster or user object.
pub fn refuse_foreign_members(config: &Value) -> Result<(), String> {
    let context = config["current-context"]
        .as_str()
        .ok_or("the kubeconfig names no current context")?;
    let entry = config["contexts"]
        .as_array()
        .and_then(|contexts| contexts.iter().find(|entry| entry["name"] == context))
        .map(|entry| &entry["context"])
        .ok_or("the kubeconfig names no current context entry")?;
    for (list, member_of, name_member, allowed) in [
        (
            "clusters",
            "cluster",
            "cluster",
            GIVEN_CLUSTER_MEMBERS.as_slice(),
        ),
        ("users", "user", "user", GIVEN_USER_MEMBERS.as_slice()),
    ] {
        let name = entry[name_member]
            .as_str()
            .ok_or_else(|| format!("the current context names no {member_of}"))?;
        let object = config[list]
            .as_array()
            .and_then(|entries| entries.iter().find(|entry| entry["name"] == name))
            .and_then(|entry| entry[member_of].as_object())
            .ok_or_else(|| format!("the kubeconfig names no {member_of} {name:?}"))?;
        if let Some(member) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!(
                "the kubeconfig's {member_of} member {member} is refused: kind writes only \
                 {allowed:?}, and {member} can reach another cluster or change who the client is"
            ));
        }
    }
    Ok(())
}

/// The run id of a given kubeconfig made for a cluster this lane created: its current context
/// and that context's cluster are both `kind-substrate-namespace-lane-<id>`, with one `<id>` for
/// which `substrate-namespace-lane-<id>-control-plane` is a node name.
///
/// # Errors
///
/// What did not match, naming the context and the cluster.
pub fn lane_cluster_id(config: &Value) -> Result<String, String> {
    let context = config["current-context"].as_str().unwrap_or_default();
    let cluster = config["contexts"]
        .as_array()
        .and_then(|contexts| contexts.iter().find(|entry| entry["name"] == context))
        .and_then(|entry| entry["context"]["cluster"].as_str())
        .unwrap_or_default();
    let prefix = format!("kind-{CLUSTER_PREFIX}");
    let id = context
        .strip_prefix(&prefix)
        .filter(|id| !id.is_empty() && cluster == context)
        .filter(|id| substrate_kubernetes_client::is_dns_label(&control_plane(id)));
    id.map(str::to_owned).ok_or_else(|| {
        format!(
            "the kubeconfig's current context {context:?} and its cluster {cluster:?} are not \
             both {prefix}<id> for one run id: a cluster this lane did not create, refused before \
             provisioning"
        )
    })
}

/// The control-plane node kind names for the lane cluster of run `id`.
pub fn control_plane(id: &str) -> String {
    format!("{}-control-plane", cluster_name(id))
}

/// Refuses a cluster whose node list, `nodes`, holds no control plane of the lane cluster `id`.
///
/// # Errors
///
/// The expected node and the nodes the API listed.
pub fn refuse_foreign_nodes(id: &str, nodes: &[String]) -> Result<(), String> {
    let expected = control_plane(id);
    if nodes.contains(&expected) {
        return Ok(());
    }
    let shown: Vec<&String> = nodes.iter().take(8).collect();
    Err(format!(
        "the API lists no node {expected:?}, only {} node(s) {shown:?}: a cluster this lane did \
         not create, refused before provisioning",
        nodes.len()
    ))
}

/// Lists the nodes through `admin` and refuses a cluster without lane `id`'s control plane;
/// read-only, before any write.
async fn verify_lane_nodes(admin: &substrate_kubernetes_client::Api, id: &str) -> Result<()> {
    let listed = admin
        .list(
            "/api/v1/nodes",
            "NodeList",
            substrate_kubernetes_client::ListForm::Metadata,
            100,
            4,
        )
        .await
        .map_err(|error| anyhow!("listing the nodes before provisioning: {error}"))?;
    let nodes: Vec<String> = listed
        .items
        .iter()
        .filter_map(|item| item["metadata"]["name"].as_str().map(str::to_owned))
        .collect();
    refuse_foreign_nodes(id, &nodes).map_err(|error| anyhow!(error))
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
pub(crate) fn write_private(path: &Path, config: &Value) -> Result<()> {
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
    removed: bool,
}

impl Cluster<'_> {
    /// Deletes the cluster once; why it failed, printed and returned, so the run is no record.
    fn remove(&mut self) -> Option<String> {
        use std::os::unix::process::CommandExt as _;
        if std::mem::replace(&mut self.removed, true) {
            return None;
        }
        let deleted = tool(self.kind, &self.kubeconfig)
            .args(["delete", "cluster", "--name", &self.name, "--kubeconfig"])
            .arg(&self.kubeconfig)
            .stdin(Stdio::null())
            .process_group(0)
            .status();
        if deleted
            .as_ref()
            .is_ok_and(std::process::ExitStatus::success)
        {
            return None;
        }
        let failure = format!(
            "deleting kind cluster {} failed ({deleted:?}); remove it with `kind delete cluster \
             --name {}`",
            self.name, self.name
        );
        eprintln!("namespace lane: {failure}");
        Some(failure)
    }
}

impl Drop for Cluster<'_> {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Removes the run's registry container on every exit path; built only once it started.
struct Registry<'a> {
    docker: &'a Path,
    name: String,
    kubeconfig: PathBuf,
    removed: bool,
}

impl Registry<'_> {
    /// Removes the container once; why it failed, printed and returned, so the run is no record.
    fn remove(&mut self) -> Option<String> {
        use std::os::unix::process::CommandExt as _;
        if std::mem::replace(&mut self.removed, true) {
            return None;
        }
        let removed = tool(self.docker, &self.kubeconfig)
            .args(["rm", "--force", &self.name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .process_group(0)
            .status();
        if removed
            .as_ref()
            .is_ok_and(std::process::ExitStatus::success)
        {
            return None;
        }
        let failure = format!(
            "removing the registry container {} failed ({removed:?}); remove it with `docker rm \
             --force {}`",
            self.name, self.name
        );
        eprintln!("namespace lane: {failure}");
        Some(failure)
    }
}

impl Drop for Registry<'_> {
    fn drop(&mut self) {
        self.remove();
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

/// The decision for `outcomes`, one per observation name. A run a signal `interrupted`, or one
/// that failed to remove what it created (`removal_failures`), is no record: the Chart-lane rule.
pub fn decision(
    outcomes: &[(String, Result<Outcome, String>)],
    sha: &str,
    head_after: &str,
    clean_before: bool,
    clean_after: bool,
    interrupted: bool,
    removal_failures: &[String],
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
    if interrupted {
        return Decision::NoRecord("a signal interrupted the run".to_owned());
    }
    if !removal_failures.is_empty() {
        return Decision::NoRecord(format!(
            "the run did not remove what it created: {}",
            removal_failures.join("; ")
        ));
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

/// `observations`' decision, withheld when steps 3 to 7 left a `step_fault`: a case that failed,
/// did not run, or was declared absent and reported otherwise.
pub fn with_steps(observations: Decision, step_fault: Option<String>) -> Decision {
    match (observations, step_fault) {
        (observations, None) => observations,
        (Decision::Failed(reason), Some(fault)) => Decision::Failed(format!("{reason}; {fault}")),
        (_, Some(fault)) => Decision::Failed(fault),
    }
}

/// The run, and whether it printed the record line.
#[allow(clippy::too_many_lines)] // The lane in its fixed order: cluster, registry, images, provisioning, observations, steps 3 to 7, removal.
fn run_observe(args: &ObserveArgs, stop: &AtomicBool) -> Result<(ExitCode, bool)> {
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
    let mut lane_id = None;
    let image;
    let docker_kubeconfig;
    if let Some(given) = &args.admin_kubeconfig {
        lane_id = Some(given_admin(given, &namespace, &admin_path)?);
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

    let admin = client(&admin_path)?;
    let executor_image = match (&args.executor_image, &registry) {
        (Some(given), _) => given.clone(),
        (None, Some(_)) => push_executor(args, &root, &image, &sha, &docker_kubeconfig, stop)?,
        (None, None) => bail!(
            "--admin-kubeconfig needs --executor-image: the run starts no registry to push the \
             executor to"
        ),
    };
    let gateway = observe::kind_gateway(&args.docker, &docker_kubeconfig, stop)?;
    let settings = run::Settings {
        executor_image,
        exec_image: image.clone(),
        egress_probe_address: format!("{gateway}:9"),
    };
    println!("namespace lane: building the shipped daemon for steps 3 to 7");
    let daemon = run::build_daemon(&root, stop)?;
    let driver_path = work.path().join("driver.json");
    let lane_driver_path = work.path().join("driver-lane.json");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if let Some(lane_id) = &lane_id {
        runtime.block_on(verify_lane_nodes(&admin, lane_id))?;
    }
    println!("namespace lane: the cases of steps 3 to 7:");
    for name in run::cases() {
        println!("namespace lane:   {name}");
    }
    let (outcomes, steps, mut removal_failures) = runtime.block_on(async {
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
                let driver = driver_config(&admin_config, token, &namespace);
                write_private(&driver_path, &driver)?;
                write_private(&lane_driver_path, &run::with_extension(driver, &settings))?;
                client(&driver_path)
            })
            .await;
        let (outcomes, steps) = match provisioned {
            Ok(driver) => {
                let inventory = run::inventory(&admin).await;
                let outcomes = lane.observe_all(&driver).await;
                let steps = match (inventory, fs::read(&admin_path)) {
                    (Ok(inventory), Ok(admin_config)) => {
                        let started = Instant::now();
                        let admin_config: Value =
                            serde_json::from_slice(&admin_config).unwrap_or(Value::Null);
                        let steps = run::Steps {
                            root: &root,
                            work: work.path(),
                            admin: &admin,
                            driver_config: &lane_driver_path,
                            admin_config: &admin_config,
                            settings: &settings,
                            namespace: &namespace,
                            unlabelled: &unlabelled,
                            inventory: &inventory,
                            stop,
                        }
                        .run(&daemon)
                        .await;
                        println!(
                            "namespace lane: steps 3 to 7 took {} s",
                            started.elapsed().as_secs()
                        );
                        run::verdict(&steps)
                    }
                    (Err(error), _) => Some(format!("the inventory was not read: {error:#}")),
                    (_, Err(error)) => Some(format!("the admin kubeconfig was not read: {error}")),
                };
                (outcomes, steps)
            }
            Err(error) => (
                OBSERVATIONS
                    .iter()
                    .map(|name| {
                        (
                            (*name).to_owned(),
                            Err(format!("provisioning failed: {error:#}")),
                        )
                    })
                    .collect(),
                Some(format!("provisioning failed: {error:#}")),
            ),
        };
        let removal_failures = if cluster.is_none() {
            lane.remove_namespaces().await
        } else {
            Vec::new()
        };
        (outcomes, steps, removal_failures)
    });
    removal_failures.extend(registry.as_mut().and_then(Registry::remove));
    removal_failures.extend(cluster.as_mut().and_then(Cluster::remove));
    finish(
        &root,
        &outcomes,
        steps,
        &sha,
        clean_before,
        &removal_failures,
        stop,
    )
}

/// Builds the executor image (Dockerfile target `executor`) from this checkout and pushes it to
/// the run's registry, beside `exec_image`; the pushed reference by digest.
fn push_executor(
    args: &ObserveArgs,
    root: &Path,
    exec_image: &str,
    sha: &str,
    kubeconfig: &Path,
    stop: &AtomicBool,
) -> Result<String> {
    let host = exec_image
        .split_once('/')
        .map(|(host, _)| host)
        .ok_or_else(|| anyhow!("{exec_image} names no registry"))?;
    let tag = format!("{host}/substrate-executor:lane");
    println!("namespace lane: building the executor image {tag} from {sha}");
    output(
        tool(&args.docker, kubeconfig)
            .current_dir(root)
            .args(["build", "--target", "executor", "--build-arg"])
            .arg(format!("SOURCE_SHA={sha}"))
            .args(["--tag", &tag, "."]),
        stop,
    )?;
    let pushed = output(tool(&args.docker, kubeconfig).args(["push", &tag]), stop)?;
    let digest =
        pushed_digest(&pushed).ok_or_else(|| anyhow!("docker push named no digest: {pushed}"))?;
    Ok(format!("{host}/substrate-executor@{digest}"))
}

/// The `sha256:<hex>` digest `docker push` reports.
pub fn pushed_digest(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let digest = line.split("digest: ").nth(1)?.split_whitespace().next()?;
        let hex = digest.strip_prefix("sha256:")?;
        (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| digest.to_owned())
    })
}

/// The run's summary, one line per observation, then the record line or why there is none; and
/// whether it printed the record. The record is printed only after the removal succeeded and
/// when no signal arrived before it.
fn finish(
    root: &Path,
    outcomes: &[(String, Result<Outcome, String>)],
    steps: Option<String>,
    sha: &str,
    clean_before: bool,
    removal_failures: &[String],
    stop: &AtomicBool,
) -> Result<(ExitCode, bool)> {
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
    match with_steps(
        decision(
            outcomes,
            sha,
            &head_after,
            clean_before,
            clean_after,
            stop.load(Ordering::SeqCst),
            removal_failures,
        ),
        steps,
    ) {
        Decision::Record(count) => {
            println!("{}", record_line(TRAILER, sha, count, SystemTime::now()));
            Ok((ExitCode::SUCCESS, true))
        }
        Decision::NoRecord(reason) => {
            eprintln!("namespace lane: every observation ran, but no record: {reason}");
            Ok((ExitCode::FAILURE, false))
        }
        Decision::Failed(reason) => {
            eprintln!("namespace lane: no record: {reason}");
            Ok((ExitCode::FAILURE, false))
        }
    }
}

/// Copies the given JSON admin kubeconfig to `admin_path`, its context naming `namespace`, and
/// returns the run id of the lane cluster it names. Refused before it is read when it is a default
/// kubeconfig (under `~/.kube` or named by `KUBECONFIG`); then for any server but kind's loopback
/// endpoint, any cluster or user member kind does not write, and any context or cluster name but
/// `kind-substrate-namespace-lane-<id>`.
fn given_admin(given: &Path, namespace: &str, admin_path: &Path) -> Result<String> {
    let homes = home_directories(std::env::var_os("HOME").map(PathBuf::from), passwd_home())
        .map_err(|error| anyhow!(error))?;
    refuse_default_kubeconfigs(given, &homes, std::env::var_os("KUBECONFIG").as_deref())
        .map_err(|error| anyhow!(error))?;
    let bytes =
        substrate_kubernetes_client::kubeconfig::read(given).map_err(|error| anyhow!(error))?;
    let mut config: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("{} is not a JSON kubeconfig", given.display()))?;
    loopback_port(current_server(&config).map_err(|error| anyhow!(error))?)
        .map_err(|error| anyhow!(error))?;
    refuse_foreign_members(&config).map_err(|error| anyhow!(error))?;
    let id = lane_cluster_id(&config).map_err(|error| anyhow!(error))?;
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
    write_private(admin_path, &config)?;
    Ok(id)
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
        removed: false,
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
        removed: false,
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

    /// O10: the driver's `pods/exec` WebSocket is refused 403 without `get` on `pods/exec`, so the
    /// Role the lane installs and decision 3's table both hold `get` beside `create` there
    /// (ADR 0047's addendum of 2026-10-29), and a table with only `create` is refused.
    #[test]
    fn the_role_holds_get_and_create_on_pods_exec_and_create_alone_is_refused() {
        let exec = |grants: &[render::Grant]| {
            grants
                .iter()
                .find(|grant| grant.group.is_empty() && grant.resource == "pods/exec")
                .map(|grant| grant.verbs.clone())
        };
        let both = Some(vec!["get".to_owned(), "create".to_owned()]);
        assert_eq!(exec(&render::role_grants()), both);
        let root = crate::repo::root().expect("root");
        let adr = fs::read_to_string(root.join(ADR)).expect("ADR");
        assert_eq!(exec(&render::adr_role_table(&adr).expect("table")), both);
        assert!(role_failures(&adr).is_empty());
        let narrowed = adr.replacen(
            "| `pods/exec` | `get`, `create` |",
            "| `pods/exec` | `create` |",
            1,
        );
        assert_ne!(narrowed, adr);
        let failures = role_failures(&narrowed);
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
        assert_eq!(
            decision(&ran, "a", "a", true, true, false, &[]),
            Decision::Record(8)
        );
        assert!(matches!(
            decision(&ran, "a", "b", true, true, false, &[]),
            Decision::NoRecord(_)
        ));
        assert!(matches!(
            decision(&ran, "a", "a", false, true, false, &[]),
            Decision::NoRecord(_)
        ));
        assert!(matches!(
            decision(&ran, "a", "a", true, false, false, &[]),
            Decision::NoRecord(_)
        ));
        let Decision::NoRecord(interrupted) = decision(&ran, "a", "a", true, true, true, &[])
        else {
            panic!("a run a signal interrupted after its last observation printed a record");
        };
        assert!(interrupted.contains("signal"), "{interrupted}");
        let removal = ["deleting namespace snl-x: the API did not answer".to_owned()];
        let Decision::NoRecord(unremoved) = decision(&ran, "a", "a", true, true, false, &removal)
        else {
            panic!("a run that failed to remove what it created printed a record");
        };
        assert!(unremoved.contains("snl-x"), "{unremoved}");
        let mut short = ran;
        short.retain(|(name, _)| name != "O10");
        assert!(matches!(
            decision(&short, "a", "a", true, true, false, &[]),
            Decision::Failed(_)
        ));
        let failed = all(|_| Err("no cluster".to_owned()));
        assert!(matches!(
            decision(&failed, "a", "a", true, true, false, &[]),
            Decision::Failed(_)
        ));
    }

    #[test]
    fn the_pushed_executor_is_named_by_the_digest_docker_push_reports() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let output = format!("lane: digest: {digest} size: 528\n");
        assert_eq!(pushed_digest(&output), Some(digest));
        assert_eq!(pushed_digest("lane: digest: sha256:abc size: 1"), None);
        assert_eq!(pushed_digest("pushed"), None);
    }

    #[test]
    fn a_step_case_that_did_not_pass_or_was_not_declared_absent_withholds_the_record() {
        assert_eq!(with_steps(Decision::Record(8), None), Decision::Record(8));
        let Decision::Failed(reason) = with_steps(
            Decision::Record(8),
            Some("S4 clean-room runner is declared absent and must be reported so".to_owned()),
        ) else {
            panic!("a run whose step 4 was reported passed printed a record");
        };
        assert!(reason.contains("S4"), "{reason}");
        assert!(matches!(
            with_steps(
                Decision::NoRecord("HEAD moved".to_owned()),
                Some("S5 failed".to_owned())
            ),
            Decision::Failed(_)
        ));
        assert_eq!(
            with_steps(Decision::NoRecord("HEAD moved".to_owned()), None),
            Decision::NoRecord("HEAD moved".to_owned())
        );
    }

    #[test]
    fn a_failed_cluster_or_registry_removal_is_returned_once_and_withholds_the_record() {
        let tools = tempfile::tempdir().expect("tools");
        let failing = tools.path().join("failing");
        fs::write(&failing, "#!/bin/sh\nexit 1\n").expect("stand-in");
        fs::set_permissions(
            &failing,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("mode");
        let kubeconfig = tools.path().join("kubeconfig");
        let mut cluster = Cluster {
            kind: &failing,
            name: cluster_name("x"),
            kubeconfig: kubeconfig.clone(),
            removed: false,
        };
        let failure = cluster.remove().expect("a failed delete is returned");
        assert!(failure.contains("kind delete cluster --name"), "{failure}");
        assert_eq!(cluster.remove(), None, "removal runs once");
        let mut registry = Registry {
            docker: &failing,
            name: "r".to_owned(),
            kubeconfig,
            removed: false,
        };
        assert!(registry.remove().is_some());
        let outcomes: Vec<(String, Result<Outcome, String>)> = OBSERVATIONS
            .iter()
            .map(|name| ((*name).to_owned(), Ok(Outcome::Held(String::new()))))
            .collect();
        assert!(matches!(
            decision(&outcomes, "a", "a", true, true, false, &[failure]),
            Decision::NoRecord(_)
        ));
    }

    #[test]
    fn the_home_kube_refusal_holds_from_the_passwd_entry_when_home_is_unset_or_elsewhere() {
        let passwd = tempfile::tempdir().expect("passwd home");
        let elsewhere = tempfile::tempdir().expect("HOME");
        let kube = passwd.path().join(".kube").join("config");
        for env_home in [
            None,
            Some(elsewhere.path().to_path_buf()),
            Some(PathBuf::new()),
        ] {
            let homes = home_directories(env_home.clone(), Some(passwd.path().to_path_buf()))
                .expect("homes");
            let refused = refuse_default_kubeconfigs(&kube, &homes, None)
                .expect_err("the passwd home's .kube");
            assert!(refused.contains("~/.kube"), "{env_home:?}: {refused}");
        }
        assert!(home_directories(None, None).is_err());
        assert!(home_directories(Some(PathBuf::new()), None).is_err());
        let own = passwd_home().expect("the current uid has a passwd entry");
        assert!(own.is_absolute(), "{}", own.display());
    }

    #[test]
    fn only_a_lane_cluster_kubeconfig_names_a_run_id() {
        let config = |context: &str, cluster: &str| {
            json!({
                "current-context": context,
                "contexts": [{"name": context, "context": {"cluster": cluster, "user": "u"}}],
            })
        };
        let lane = "kind-substrate-namespace-lane-0a1b2c3d4e5f";
        assert_eq!(
            lane_cluster_id(&config(lane, lane)),
            Ok("0a1b2c3d4e5f".to_owned())
        );
        for (context, cluster) in [
            ("kind-b10x-dev", "kind-b10x-dev"),
            (lane, "kind-b10x-dev"),
            ("kind-b10x-dev", lane),
            (lane, "kind-substrate-namespace-lane-ffffffffffff"),
            (
                "kind-substrate-namespace-lane-",
                "kind-substrate-namespace-lane-",
            ),
            ("substrate-namespace-lane-x", "substrate-namespace-lane-x"),
            (
                "kind-substrate-namespace-lane-A",
                "kind-substrate-namespace-lane-A",
            ),
            (
                "kind-substrate-namespace-lane-x.y",
                "kind-substrate-namespace-lane-x.y",
            ),
        ] {
            let refused = lane_cluster_id(&config(context, cluster)).expect_err(context);
            assert!(refused.contains("this lane did not create"), "{refused}");
            assert!(
                refused.contains(context) && refused.contains(cluster),
                "{refused}"
            );
        }
        let long = format!("kind-{}", cluster_name(&"a".repeat(40)));
        assert!(lane_cluster_id(&config(&long, &long)).is_err());
    }

    #[test]
    fn a_cluster_without_the_lane_runs_control_plane_node_is_refused() {
        let id = "0a1b2c3d4e5f";
        let own = vec![
            "substrate-namespace-lane-0a1b2c3d4e5f-control-plane".to_owned(),
            "substrate-namespace-lane-0a1b2c3d4e5f-worker".to_owned(),
        ];
        assert_eq!(refuse_foreign_nodes(id, &own), Ok(()));
        for nodes in [
            vec!["b10x-dev-control-plane".to_owned()],
            vec!["substrate-namespace-lane-ffffffffffff-control-plane".to_owned()],
            vec!["substrate-namespace-lane-0a1b2c3d4e5f-worker".to_owned()],
            Vec::new(),
        ] {
            let refused = refuse_foreign_nodes(id, &nodes).expect_err("foreign");
            assert!(
                refused.contains("substrate-namespace-lane-0a1b2c3d4e5f-control-plane"),
                "{refused}"
            );
            assert!(refused.contains("this lane did not create"), "{refused}");
        }
    }
}
