//! The Kubernetes serving profile's chart: its static lane and its release packager (ADR 0046).
//!
//! `cargo xtask chart-lane static` renders `deploy/helm/substrate` with the pinned `helm` and holds
//! every rendering to ADR 0046: one `StatefulSet` behind one headless Service and nothing else that
//! selects its pods, one owner-private ledger claim per ordinal kept on deletion and scale-down,
//! certificate names that are per-pod hosts and never the Service, a token-less `ServiceAccount` with
//! no Role, a default-deny `NetworkPolicy`, the K3 init containers, the K8 probes, and the K4 pod
//! posture (design 25's five capabilities only with `execution.enabled`, no privileged container,
//! no host namespace, no host path). It then requires `helm template` to refuse every privilege,
//! cgroup, mount, quota and transport posture the chart does not serve. The cluster scenarios are
//! `cargo xtask chart-lane cluster` (`chart_cluster.rs`).
//!
//! `cargo xtask package-chart` writes the released chart: the source chart with its version set
//! and its default image pinned to the published daemon digest, packaged by `helm package`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::chart_yaml;

/// The chart, relative to the repository root.
pub const CHART: &str = "deploy/helm/substrate";
/// The five capabilities design 25's bootstrap starts with, and the only ones a daemon pod adds.
pub const BOOTSTRAP_CAPABILITIES: [&str; 5] = ["CHOWN", "SETUID", "SETGID", "SETPCAP", "SYS_ADMIN"];
const STATE_MOUNT: &str = "/var/lib/b10x-substrate";
const STATE_PATH: &str = "/var/lib/b10x-substrate/state/state.db";
const SECCOMP_PROFILE: &str = "b10x-substrate/host-exec-v1-amd64.json";
const APPARMOR_PROFILE: &str = "substrate-host-exec-v2";
/// How the bootstrap and the installer are told a profile name other than the default (design 33).
const PROFILE_FLAG: &str = "--apparmor-profile";
const RELEASE: &str = "lane";
const NAMESPACE: &str = "substrate-lane";
const NODE_PROFILES_NAMESPACE: &str = "substrate-node-profiles";
const LANE_DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, ClapArgs)]
pub struct LaneArgs {
    #[command(subcommand)]
    lane: Lane,
}

#[derive(Debug, Subcommand)]
enum Lane {
    /// Render the chart and hold every rendering and refusal to ADR 0046; needs only `helm`.
    Static(StaticArgs),
    /// Run the install, upgrade and uninstall scenarios against a prepared kind cluster.
    Cluster(crate::chart_cluster::Args),
}

#[derive(Debug, ClapArgs)]
pub struct StaticArgs {
    /// The `helm` executable.
    #[arg(long, default_value = "helm")]
    helm: PathBuf,
}

pub fn run_lane(args: &LaneArgs) -> Result<ExitCode> {
    match &args.lane {
        Lane::Static(args) => run_static(args),
        Lane::Cluster(args) => {
            crate::chart_cluster::arm_signals()?;
            crate::chart_cluster::run(args)
        }
    }
}

/// The values every lane rendering starts from: a complete configuration with execution off.
pub fn base_values() -> Value {
    json!({
        "image": {"digest": LANE_DIGEST},
        "nodePool": {
            "label": {"key": "b10x.dev/substrate-pool", "value": "lane"},
            "taint": {"key": "b10x.dev/substrate-pool", "value": "lane", "effect": "NoSchedule"}
        },
        "persistence": {"storageClassName": "standard"},
        "tls": {"secretName": "substrate-tls"},
        "identity": {
            "origin": "https://identity.example.test",
            "caConfigMap": {"name": "identity-ca"},
            "egress": {"cidr": "192.0.2.10/32", "port": 443}
        },
        "networkPolicy": {
            "clients": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "clients"}}}],
            "scrapers": [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": "scraper"}}}]
        },
        "daemon": {
            "gitSources": [{"name": "forge", "urlPrefix": "https://git.example.test/org/", "cidr": "192.0.2.20/32", "port": 443}],
            "egressApertures": [{"name": "api", "host": "api.example.test", "port": 443, "maxBytes": "16MiB", "cidr": "192.0.2.30/32"}]
        }
    })
}

/// One rendering the chart must accept, and what its manifests must then show.
pub struct Accepted {
    pub name: &'static str,
    pub values: Value,
    pub expect: Expect,
}

/// What a rendering's manifests must show beyond the invariants every rendering holds.
#[derive(Clone)]
pub struct Expect {
    pub namespace: String,
    pub domain: String,
    pub replicas: usize,
    pub execution: bool,
    pub quotas: bool,
    pub node_profiles: Option<String>,
    /// The `AppArmor` profile the values name (design 33).
    pub apparmor_profile: String,
    pub allow_any_egress: bool,
    /// The dedicated pool: the node label `(key, value)` and the taint `(key, value, effect)`.
    pub pool: Pool,
    /// The `(cidr, port)` destinations the values declare; `None` checks only each rule's shape.
    pub egress: Option<BTreeSet<(String, u64)>>,
}

#[derive(Clone)]
pub struct Pool {
    pub label: (String, String),
    pub taint: (String, String, String),
}

impl Pool {
    /// The pool the values name.
    pub fn of(values: &Value) -> Self {
        let field = |pointer: &str| text(values, pointer).to_owned();
        Self {
            label: (field("/nodePool/label/key"), field("/nodePool/label/value")),
            taint: (
                field("/nodePool/taint/key"),
                field("/nodePool/taint/value"),
                field("/nodePool/taint/effect"),
            ),
        }
    }
}

impl Expect {
    fn lane() -> Self {
        Self {
            namespace: NAMESPACE.to_owned(),
            domain: "cluster.local".to_owned(),
            replicas: 2,
            execution: false,
            quotas: false,
            node_profiles: None,
            apparmor_profile: APPARMOR_PROFILE.to_owned(),
            allow_any_egress: false,
            pool: Pool::of(&base_values()),
            egress: None,
        }
    }
}

/// Every `(cidr, port)` destination the values declare: Identity, each Git source, each aperture.
pub fn declared_egress(values: &Value) -> BTreeSet<(String, u64)> {
    let destination = |entry: &Value| {
        (
            text(entry, "/cidr").to_owned(),
            entry["port"].as_u64().unwrap_or(0),
        )
    };
    std::iter::once(destination(&values["identity"]["egress"]))
        .chain(array(values, "/daemon/gitSources").iter().map(destination))
        .chain(
            array(values, "/daemon/egressApertures")
                .iter()
                .map(destination),
        )
        .collect()
}

/// A CIDR whose prefix length is zero admits every address, however its address part is spelled.
fn admits_every_address(cidr: &str) -> bool {
    cidr.rsplit_once('/').is_none_or(|(_, prefix)| {
        prefix.trim().parse::<u32>().is_ok_and(|length| length == 0)
            || !prefix.trim().bytes().all(|byte| byte.is_ascii_digit())
            || prefix.trim().is_empty()
    })
}

pub fn accepted_cases() -> Vec<Accepted> {
    let lane = Expect::lane();
    vec![
        Accepted {
            name: "execution off",
            values: json!({}),
            expect: lane.clone(),
        },
        Accepted {
            name: "execution on, node profiles in their own namespace",
            values: json!({
                "replicas": 3,
                "execution": {"enabled": true},
                "nodeProfiles": {"enabled": true, "namespace": NODE_PROFILES_NAMESPACE}
            }),
            expect: Expect {
                replicas: 3,
                execution: true,
                node_profiles: Some(NODE_PROFILES_NAMESPACE.to_owned()),
                ..lane.clone()
            },
        },
        Accepted {
            name: "execution, project quotas and node profiles under a named AppArmor profile",
            values: json!({
                "execution": {
                    "enabled": true,
                    "appArmorProfile": "substrate-host-exec-v1-lane-01k8xyz",
                    "projectQuotas": {"enabled": true, "ids": "200000-200511"}
                },
                "persistence": {"projectQuotaCapable": true},
                "nodeProfiles": {"enabled": true, "namespace": NODE_PROFILES_NAMESPACE}
            }),
            expect: Expect {
                execution: true,
                quotas: true,
                node_profiles: Some(NODE_PROFILES_NAMESPACE.to_owned()),
                apparmor_profile: "substrate-host-exec-v1-lane-01k8xyz".to_owned(),
                ..lane.clone()
            },
        },
        Accepted {
            name: "execution and node profiles under the v1 policy a node already holds",
            values: json!({
                "execution": {"enabled": true, "appArmorProfile": "substrate-host-exec-v1"},
                "nodeProfiles": {"enabled": true, "namespace": NODE_PROFILES_NAMESPACE}
            }),
            expect: Expect {
                execution: true,
                node_profiles: Some(NODE_PROFILES_NAMESPACE.to_owned()),
                apparmor_profile: "substrate-host-exec-v1".to_owned(),
                ..lane.clone()
            },
        },
        Accepted {
            name: "execution with project quotas on a declared quota-capable class",
            values: json!({
                "execution": {"enabled": true, "projectQuotas": {"enabled": true, "ids": "200000-200511"}},
                "persistence": {"projectQuotaCapable": true}
            }),
            expect: Expect {
                execution: true,
                quotas: true,
                ..lane.clone()
            },
        },
        Accepted {
            name: "any-address egress, explicitly admitted, and another cluster domain",
            values: json!({
                "replicas": 1,
                "clusterDomain": "edge.internal",
                "networkPolicy": {"allowAnyEgress": true},
                "daemon": {"egressApertures": [{"name": "open", "host": "example.test", "port": 443, "cidr": "0.0.0.0/0"}]}
            }),
            expect: Expect {
                replicas: 1,
                domain: "edge.internal".to_owned(),
                allow_any_egress: true,
                ..lane
            },
        },
    ]
}

/// The class of posture a refused rendering attempts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Posture {
    Privilege,
    HostNamespace,
    HostPath,
    Cgroup,
    Mount,
    Quota,
    Transport,
    Egress,
    Image,
    NodeProfiles,
}

/// One rendering the chart must refuse. `message` is a fragment of a refusal the chart's own
/// template writes; schema refusals carry none, because their wording is the Helm release's.
pub struct Refused {
    pub name: &'static str,
    pub posture: Posture,
    pub values: Value,
    pub message: Option<&'static str>,
}

#[allow(clippy::too_many_lines)] // One table of refusals, one row each, so a reader can check them by eye.
pub fn refused_cases() -> Vec<Refused> {
    let schema = |name, posture, values| Refused {
        name,
        posture,
        values,
        message: None,
    };
    let template = |name, posture, values, message| Refused {
        name,
        posture,
        values,
        message: Some(message),
    };
    vec![
        schema(
            "privileged at the top",
            Posture::Privilege,
            json!({"privileged": true}),
        ),
        schema(
            "a container security context",
            Posture::Privilege,
            json!({"securityContext": {"privileged": true}}),
        ),
        schema(
            "privileged execution",
            Posture::Privilege,
            json!({"execution": {"privileged": true}}),
        ),
        schema(
            "added capabilities",
            Posture::Privilege,
            json!({"execution": {"capabilities": ["NET_ADMIN"]}}),
        ),
        schema(
            "host network",
            Posture::HostNamespace,
            json!({"hostNetwork": true}),
        ),
        schema("host PID", Posture::HostNamespace, json!({"hostPID": true})),
        schema(
            "execution in the host PID namespace",
            Posture::HostNamespace,
            json!({"execution": {"hostPID": true}}),
        ),
        schema(
            "a host path for the daemon",
            Posture::HostPath,
            json!({"execution": {"hostPath": "/var/lib"}}),
        ),
        schema(
            "extra volumes",
            Posture::HostPath,
            json!({"extraVolumes": [{"name": "host", "hostPath": {"path": "/"}}]}),
        ),
        schema(
            "a cgroup root override",
            Posture::Cgroup,
            json!({"daemon": {"cgroupRoot": "/sys/fs/cgroup"}}),
        ),
        schema(
            "a writable host root",
            Posture::Mount,
            json!({"daemon": {"writableRoots": [{"name": "cache", "path": "/cache"}]}}),
        ),
        schema(
            "a read-only host root",
            Posture::Mount,
            json!({"daemon": {"readOnlyRoots": ["/srv"]}}),
        ),
        template(
            "project quotas without execution",
            Posture::Quota,
            json!({"execution": {"projectQuotas": {"enabled": true, "ids": "1-200"}}, "persistence": {"projectQuotaCapable": true}}),
            "requires execution.enabled",
        ),
        template(
            "project quotas without a declared quota-capable class",
            Posture::Quota,
            json!({"execution": {"enabled": true, "projectQuotas": {"enabled": true, "ids": "1-200"}}}),
            "requires persistence.projectQuotaCapable",
        ),
        template(
            "project quotas without an identity range",
            Posture::Quota,
            json!({"execution": {"enabled": true, "projectQuotas": {"enabled": true}}, "persistence": {"projectQuotaCapable": true}}),
            "requires execution.projectQuotas.ids",
        ),
        schema(
            "the development TCP listener",
            Posture::Transport,
            json!({"daemon": {"tcpListen": "0.0.0.0:8080"}}),
        ),
        schema(
            "an allowed local uid",
            Posture::Transport,
            json!({"daemon": {"allowUid": [0]}}),
        ),
        schema(
            "no TLS certificate",
            Posture::Transport,
            json!({"tls": {"secretName": ""}}),
        ),
        schema(
            "a plaintext Identity origin",
            Posture::Transport,
            json!({"identity": {"origin": "http://identity.example.test"}}),
        ),
        schema(
            "no Identity trust roots",
            Posture::Transport,
            json!({"identity": {"caConfigMap": {"name": ""}}}),
        ),
        schema(
            "an Identity destination without a CIDR",
            Posture::Egress,
            json!({"identity": {"egress": {"cidr": "", "port": 443}}}),
        ),
        schema(
            "an aperture without a CIDR",
            Posture::Egress,
            json!({"daemon": {"egressApertures": [{"name": "api", "host": "api.example.test", "port": 443}]}}),
        ),
        template(
            "any-address egress without the explicit admission",
            Posture::Egress,
            json!({"identity": {"egress": {"cidr": "0.0.0.0/0", "port": 443}}}),
            "admits every address",
        ),
        template(
            "an image named by neither digest nor tag",
            Posture::Image,
            json!({"image": {"digest": ""}}),
            "image.digest or image.tag",
        ),
        template(
            "node profiles without a namespace",
            Posture::NodeProfiles,
            json!({"nodeProfiles": {"enabled": true}}),
            "requires nodeProfiles.namespace",
        ),
        template(
            "node profiles in the release namespace",
            Posture::NodeProfiles,
            json!({"nodeProfiles": {"enabled": true, "namespace": NAMESPACE}}),
            "must differ from the release namespace",
        ),
        template(
            "security review: any-address IPv4 egress spelled with a host part",
            Posture::Egress,
            json!({"identity": {"egress": {"cidr": "10.0.0.0/0", "port": 443}}}),
            "admits every address",
        ),
        template(
            "security review: any-address IPv6 egress spelled long",
            Posture::Egress,
            json!({"daemon": {"egressApertures": [{"name": "api", "host": "api.example.test", "port": 443, "cidr": "0::/0"}]}}),
            "admits every address",
        ),
        schema(
            "security review: an AppArmor-unconfined pod annotation",
            Posture::Privilege,
            json!({"podAnnotations": {"container.apparmor.security.beta.kubernetes.io/daemon": "unconfined"}}),
        ),
        schema(
            "a seccomp profile annotation",
            Posture::Privilege,
            json!({"podAnnotations": {"seccomp.security.alpha.kubernetes.io/pod": "unconfined"}}),
        ),
        schema(
            "an AppArmor profile outside the execution profile's family",
            Posture::Privilege,
            json!({"execution": {"enabled": true, "appArmorProfile": "unconfined"}}),
        ),
        schema(
            "an IPv6 prefix length spelled with a leading zero",
            Posture::Egress,
            json!({"identity": {"egress": {"cidr": "::/00", "port": 443}}}),
        ),
    ]
}

/// Values the schema alone refuses today. Rendered with `--skip-schema-validation`, each must be
/// refused by the templates themselves, or render nothing ADR 0046 § 5 and K4 forbid.
pub fn injected_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a node-pool label key that opens the host namespaces",
            json!({"nodePool": {"label": {"key": "pool: \"a\"\n      hostPID: true\n      hostNetwork: true\n      hostIPC: true\n      nodeSelector:\n        b10x.dev/substrate-pool", "value": "lane"}}}),
        ),
        (
            "a TLS secret name that adds a host path volume",
            json!({"tls": {"secretName": "substrate-tls\n        - name: host-root\n          hostPath:\n            path: /\n        - name: spare\n          secret:\n            secretName: substrate-tls"}}),
        ),
        (
            "an AppArmor-unconfined annotation",
            json!({"podAnnotations": {"container.apparmor.security.beta.kubernetes.io/daemon": "unconfined"}}),
        ),
        (
            "an any-address prefix spelled with a leading zero",
            json!({"identity": {"egress": {"cidr": "::/00", "port": 443}}}),
        ),
        (
            "a storage class name that adds a key",
            json!({"persistence": {"storageClassName": "standard\n        volumeMode: Block"}}),
        ),
        (
            "an AppArmor profile name that unconfines the daemon and adds a command word",
            json!({"execution": {"enabled": true, "appArmorProfile": "unconfined\n            - --allow-uid"},
                   "nodeProfiles": {"enabled": true, "namespace": NODE_PROFILES_NAMESPACE}}),
        ),
        (
            "an Identity CA key that adds a command word",
            json!({"identity": {"caConfigMap": {"name": "identity-ca", "key": "ca.crt\n            - /etc/passwd"}}}),
        ),
    ]
}

/// `command`, a Helm that reaches no cluster: an empty kubeconfig of its own in `directory` and
/// none of the ambient `HELM_KUBE*` overrides. A workstation's ambient contexts may be production
/// clusters, and rendering or linting the chart needs none.
fn without_cluster<'c>(command: &'c mut Command, directory: &Path) -> Result<&'c mut Command> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let kubeconfig = directory.join("kubeconfig");
    // Owner-only, so Helm does not warn that a group- or world-readable kubeconfig is insecure.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&kubeconfig)?;
    command.env("KUBECONFIG", &kubeconfig);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("HELM_KUBE") {
            command.env_remove(name);
        }
    }
    Ok(command)
}

fn render_skipping_schema(
    helm: &Path,
    chart: &Path,
    values: &Value,
) -> Result<std::result::Result<String, String>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("values.json");
    fs::write(&file, serde_json::to_vec_pretty(values)?)?;
    let output = without_cluster(&mut Command::new(helm), directory.path())?
        .args(["template", RELEASE])
        .arg(chart)
        .args([
            "--namespace",
            NAMESPACE,
            "--skip-schema-validation",
            "--values",
        ])
        .arg(&file)
        .output()
        .with_context(|| format!("running {}", helm.display()))?;
    Ok(if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    })
}

/// `overlay` merged into `base`: objects merge key by key, anything else replaces.
pub fn merged(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            let mut result = base.clone();
            for (key, value) in overlay {
                let merged_value = result
                    .get(key)
                    .map_or_else(|| value.clone(), |existing| merged(existing, value));
                result.insert(key.clone(), merged_value);
            }
            Value::Object(result)
        }
        _ => overlay.clone(),
    }
}

/// `helm template` of the chart with `values`; `Ok(Err(stderr))` when Helm refused.
pub fn render(
    helm: &Path,
    chart: &Path,
    release: &str,
    namespace: &str,
    values: &Value,
) -> Result<std::result::Result<String, String>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("values.json");
    fs::write(&file, serde_json::to_vec_pretty(values)?)?;
    let output = without_cluster(&mut Command::new(helm), directory.path())?
        .arg("template")
        .arg(release)
        .arg(chart)
        .args(["--namespace", namespace, "--values"])
        .arg(&file)
        .output()
        .with_context(|| format!("running {}", helm.display()))?;
    Ok(if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    })
}

/// The chart as `package-chart` archives it renders and holds to ADR 0046 like the source tree.
fn check_packaged(
    helm: &Path,
    chart: &Path,
    directory: &Path,
    failures: &mut Vec<String>,
) -> Result<()> {
    let packaged = directory.join("packaged");
    match package_archive(chart, "0.0.1", LANE_DIGEST) {
        Err(error) => failures.push(format!("package-chart: {error:#}")),
        Ok(archive) => {
            fs::create_dir_all(&packaged)?;
            let file = packaged.join("b10x-substrate-0.0.1.tgz");
            fs::write(&file, archive)?;
            let mut values = base_values();
            values["image"] = json!({});
            let expect = Expect {
                egress: Some(declared_egress(&values)),
                ..Expect::lane()
            };
            match render(helm, &file, RELEASE, NAMESPACE, &values)? {
                Err(stderr) => failures.push(format!(
                    "package-chart: helm does not render the packaged archive: {stderr}"
                )),
                Ok(text) => failures.extend(
                    check_manifests(&chart_yaml::documents(&text)?, &expect)
                        .into_iter()
                        .map(|failure| format!("package-chart: {failure}")),
                ),
            }
        }
    }
    Ok(())
}

/// Each injected value, rendered with the schema skipped, is refused by the templates or renders
/// nothing ADR 0046 forbids, in YAML the API decodes as the lane reads it.
fn check_injected(helm: &Path, chart: &Path, failures: &mut Vec<String>) -> Result<()> {
    for (name, overlay) in injected_cases() {
        let values = merged(&base_values(), &overlay);
        if let Ok(text) = render_skipping_schema(helm, chart, &values)? {
            match chart_yaml::documents(&text) {
                Err(error) => failures.push(format!(
                    "{name}: with the schema skipped the templates rendered YAML the API decodes last-key-wins: {error:#}"
                )),
                Ok(documents) => {
                    let found = check_manifests(&documents, &Expect::lane());
                    if !found.is_empty() {
                        failures.push(format!(
                            "{name}: with the schema skipped the templates rendered {found:?}"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn run_static(args: &StaticArgs) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let chart = root.join(CHART);
    let mut failures = Vec::new();

    let directory = tempfile::tempdir()?;
    let base_file = directory.path().join("lane.json");
    fs::write(&base_file, serde_json::to_vec_pretty(&base_values())?)?;
    let lint = without_cluster(&mut Command::new(&args.helm), directory.path())?
        .args(["lint", "--strict"])
        .arg(&chart)
        .arg("--values")
        .arg(&base_file)
        .output()
        .with_context(|| format!("running {}", args.helm.display()))?;
    if !lint.status.success() {
        failures.push(format!(
            "helm lint --strict refused the lane values:\n{}{}",
            String::from_utf8_lossy(&lint.stdout),
            String::from_utf8_lossy(&lint.stderr)
        ));
    }

    let accepted = accepted_cases();
    for case in &accepted {
        let values = merged(&base_values(), &case.values);
        let expect = Expect {
            pool: Pool::of(&values),
            egress: Some(declared_egress(&values)),
            ..case.expect.clone()
        };
        match render(&args.helm, &chart, RELEASE, NAMESPACE, &values)? {
            Err(stderr) => {
                failures.push(format!("{}: helm template refused it: {stderr}", case.name));
            }
            Ok(text) => match chart_yaml::documents(&text) {
                Err(error) => failures.push(format!("{}: rendered YAML: {error:#}", case.name)),
                Ok(documents) => failures.extend(
                    check_manifests(&documents, &expect)
                        .into_iter()
                        .map(|failure| format!("{}: {failure}", case.name)),
                ),
            },
        }
    }

    check_packaged(&args.helm, &chart, directory.path(), &mut failures)?;

    let refused = refused_cases();
    for case in &refused {
        let values = merged(&base_values(), &case.values);
        match render(&args.helm, &chart, RELEASE, NAMESPACE, &values)? {
            Ok(_) => failures.push(format!(
                "{} ({:?}): helm template rendered what the chart must refuse",
                case.name, case.posture
            )),
            Err(stderr) => {
                if let Some(message) = case.message
                    && !stderr.contains(message)
                {
                    failures.push(format!(
                        "{}: refused, but not with {message:?}: {stderr}",
                        case.name
                    ));
                }
            }
        }
    }

    check_injected(&args.helm, &chart, &mut failures)?;

    if failures.is_empty() {
        println!(
            "chart lane static: helm lint --strict held; {} accepted renderings and the packaged chart checked against ADR 0046; {} postures refused; {} values injected with the schema skipped held",
            accepted.len(),
            refused.len(),
            injected_cases().len()
        );
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("chart lane static: {} failure(s)", failures.len());
        for failure in &failures {
            eprintln!("  {failure}");
        }
        Ok(ExitCode::FAILURE)
    }
}

fn text<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn array<'a>(value: &'a Value, pointer: &str) -> &'a [Value] {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn kind(document: &Value) -> &str {
    text(document, "/kind")
}

/// `selector` is a non-empty subset of `labels`.
fn selects(selector: &Value, labels: &Value) -> bool {
    selector.as_object().is_some_and(|selector| {
        !selector.is_empty()
            && selector
                .iter()
                .all(|(key, value)| labels.get(key) == Some(value))
    })
}

/// The labels the `StatefulSet` controller adds to each pod beyond its template's own.
const CONTROLLER_LABELS: [&str; 3] = [
    "statefulset.kubernetes.io/pod-name",
    "apps.kubernetes.io/pod-index",
    "controller-revision-hash",
];

/// `selector` is non-empty and every label it names is one of the pods' template `labels` with
/// that value, or one of the labels the controller adds to some pod, whatever its value.
fn may_select_pods(selector: &Value, labels: &Value) -> bool {
    selector.as_object().is_some_and(|selector| {
        !selector.is_empty()
            && selector.iter().all(|(key, value)| {
                labels.get(key) == Some(value) || CONTROLLER_LABELS.contains(&key.as_str())
            })
    })
}

/// Every failure of a rendering's manifests against ADR 0046, one line each.
pub fn check_manifests(documents: &[Value], expect: &Expect) -> Vec<String> {
    let mut failures = Vec::new();
    let allowed: BTreeSet<&str> = [
        "ServiceAccount",
        "Service",
        "ConfigMap",
        "StatefulSet",
        "NetworkPolicy",
        "DaemonSet",
    ]
    .into();
    for document in documents {
        if !allowed.contains(kind(document)) {
            failures.push(format!(
                "renders a {} named {:?}; the chart renders no RBAC, Secret, Namespace or other workload",
                kind(document),
                text(document, "/metadata/name")
            ));
        }
        let namespace = text(document, "/metadata/namespace");
        let node_profiles = expect.node_profiles.as_deref();
        if namespace != expect.namespace && Some(namespace) != node_profiles {
            failures.push(format!(
                "{} {:?} is in namespace {namespace:?}",
                kind(document),
                text(document, "/metadata/name")
            ));
        }
        if kind(document) == "ServiceAccount"
            && document.get("automountServiceAccountToken") != Some(&Value::Bool(false))
        {
            failures.push(format!(
                "ServiceAccount {:?} mounts a token",
                text(document, "/metadata/name")
            ));
        }
    }

    let statefulsets: Vec<&Value> = documents
        .iter()
        .filter(|document| kind(document) == "StatefulSet")
        .collect();
    let [statefulset] = statefulsets.as_slice() else {
        failures.push(format!(
            "renders {} StatefulSets, not one",
            statefulsets.len()
        ));
        return failures;
    };
    failures.extend(check_statefulset(statefulset, documents, expect));
    let labels = statefulset
        .pointer("/spec/template/metadata/labels")
        .cloned()
        .unwrap_or(Value::Null);
    failures.extend(check_services(documents, statefulset, &labels));
    failures.extend(check_network_policy(documents, &labels, expect));
    failures.extend(check_certificate_names(documents, statefulset, expect));
    failures.extend(check_node_profiles(documents, expect));
    failures
}

fn check_statefulset(statefulset: &Value, documents: &[Value], expect: &Expect) -> Vec<String> {
    let mut failures = Vec::new();
    let spec = &statefulset["spec"];
    let pod = &spec["template"]["spec"];
    if spec["replicas"].as_u64() != Some(expect.replicas as u64) {
        failures.push(format!(
            "replicas is {}, not {}",
            spec["replicas"], expect.replicas
        ));
    }
    if text(spec, "/updateStrategy/type") != "RollingUpdate"
        || spec.pointer("/updateStrategy/rollingUpdate/maxUnavailable") != Some(&json!(1))
        || text(spec, "/podManagementPolicy") != "OrderedReady"
    {
        failures
            .push("the update is not a one-pod-at-a-time, readiness-gated rolling update".into());
    }
    for field in ["whenDeleted", "whenScaled"] {
        if text(
            spec,
            &format!("/persistentVolumeClaimRetentionPolicy/{field}"),
        ) != "Retain"
        {
            failures.push(format!("the ledger claim is not retained {field}"));
        }
    }
    let claims = array(spec, "/volumeClaimTemplates");
    let ledger = claims.first();
    if claims.len() != 1
        || ledger.is_none_or(|claim| {
            text(claim, "/metadata/name") != "ledger"
                || array(claim, "/spec/accessModes") != [json!("ReadWriteOnce")]
                || text(claim, "/spec/storageClassName").is_empty()
        })
    {
        failures.push("there is not exactly one ReadWriteOnce ledger claim per ordinal".into());
    }

    let labels = &spec["template"]["metadata"]["labels"];
    if !selects(&spec["selector"]["matchLabels"], labels) {
        failures.push("the StatefulSet selector does not select its own pods".into());
    }
    failures.extend(pool_binding(pod, &expect.pool, "daemon pod"));
    failures.extend(pod_annotations(&spec["template"]["metadata"], "daemon pod"));
    let anti = array(
        pod,
        "/affinity/podAntiAffinity/requiredDuringSchedulingIgnoredDuringExecution",
    );
    if !anti.iter().any(|term| {
        text(term, "/topologyKey") == "kubernetes.io/hostname"
            && selects(&term["labelSelector"]["matchLabels"], labels)
    }) {
        failures.push("no required anti-affinity keeps two daemons off one node".into());
    }
    if pod["automountServiceAccountToken"] != Value::Bool(false) {
        failures.push("the daemon pod mounts a ServiceAccount token".into());
    }
    let account = text(pod, "/serviceAccountName");
    if !documents.iter().any(|document| {
        kind(document) == "ServiceAccount" && text(document, "/metadata/name") == account
    }) {
        failures.push(format!(
            "the pod's ServiceAccount {account:?} is not the chart's own"
        ));
    }
    if pod["securityContext"]["fsGroup"] != json!(65532)
        || text(pod, "/securityContext/fsGroupChangePolicy") != "OnRootMismatch"
    {
        failures.push(
            "the pod does not set fsGroup 65532 with OnRootMismatch, so a restart could widen the 0700 state directory"
                .into(),
        );
    }
    failures.extend(pod_posture(
        pod,
        expect.execution.then_some(expect.apparmor_profile.as_str()),
        "daemon pod",
    ));
    failures.extend(check_daemon(pod, expect));
    failures.extend(check_init(pod));
    failures
}

/// ADR 0046 § 3: a required node selector on the pool's label and a toleration of the pool's taint,
/// and no toleration of anything else.
fn pool_binding(pod: &Value, pool: &Pool, what: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let (label_key, label_value) = &pool.label;
    if label_key.is_empty() || pod["nodeSelector"][label_key.as_str()] != json!(label_value) {
        failures.push(format!(
            "the {what} is not bound to the dedicated pool: its node selector does not require {label_key}={label_value}"
        ));
    }
    let (taint_key, taint_value, effect) = &pool.taint;
    let tolerations = array(pod, "/tolerations");
    let pool_taint = |toleration: &Value| {
        text(toleration, "/key") == taint_key
            && text(toleration, "/operator") == "Equal"
            && text(toleration, "/value") == taint_value
            && text(toleration, "/effect") == effect
    };
    if taint_key.is_empty() || !tolerations.iter().any(pool_taint) {
        failures.push(format!(
            "the {what} does not tolerate the dedicated pool's taint {taint_key}={taint_value}:{effect}"
        ));
    }
    for toleration in tolerations
        .iter()
        .filter(|toleration| !pool_taint(toleration))
    {
        failures.push(format!(
            "the {what} tolerates {toleration}, which is not the dedicated pool's taint, so it may schedule off the pool"
        ));
    }
    failures
}

/// K4: an annotation that selects an `AppArmor` or seccomp profile would override or unconfine what
/// the pod spec states; the API server copies such annotations into absent fields.
fn pod_annotations(metadata: &Value, what: &str) -> Vec<String> {
    metadata["annotations"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| security_profile_annotation(key))
        .map(|(key, _)| format!("the {what} carries the AppArmor or seccomp annotation {key:?}"))
        .collect()
}

/// An annotation key that selects an `AppArmor` or seccomp profile.
pub fn security_profile_annotation(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("apparmor") || key.contains("seccomp.security.alpha.kubernetes.io/")
}

/// K4: no privileged container, host namespace or host path; capabilities only as design 25 adds.
/// `execution` is the `AppArmor` profile the daemon runs under when execution is enabled.
pub fn pod_posture(pod: &Value, execution: Option<&str>, what: &str) -> Vec<String> {
    let profile = execution.unwrap_or(APPARMOR_PROFILE);
    let execution = execution.is_some();
    let mut failures = Vec::new();
    for field in ["hostNetwork", "hostPID", "hostIPC", "hostUsers"] {
        if field != "hostUsers" && pod[field] == Value::Bool(true) {
            failures.push(format!("the {what} sets {field}"));
        }
    }
    for volume in array(pod, "/volumes") {
        if volume.get("hostPath").is_some() {
            failures.push(format!(
                "the {what} mounts host path volume {:?}",
                text(volume, "/name")
            ));
        }
    }
    let pod_seccomp = text(pod, "/securityContext/seccompProfile/type");
    let containers = array(pod, "/initContainers")
        .iter()
        .map(|container| (container, false))
        .chain(
            array(pod, "/containers")
                .iter()
                .map(|container| (container, true)),
        );
    for (container, main) in containers {
        let name = text(container, "/name");
        let context = &container["securityContext"];
        if context["privileged"] == Value::Bool(true) {
            failures.push(format!("container {name} is privileged"));
        }
        if !array(context, "/capabilities/drop").contains(&json!("ALL")) {
            failures.push(format!("container {name} does not drop every capability"));
        }
        if context["readOnlyRootFilesystem"] != Value::Bool(true) {
            failures.push(format!(
                "container {name} does not set readOnlyRootFilesystem: true"
            ));
        }
        let added: BTreeSet<&str> = array(context, "/capabilities/add")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let bootstrap = execution && main && name == "daemon";
        if bootstrap {
            if added != BOOTSTRAP_CAPABILITIES.into() {
                failures.push(format!(
                    "container {name} adds {added:?}, not exactly design 25's five capabilities"
                ));
            }
            if text(context, "/seccompProfile/type") != "Localhost"
                || text(context, "/seccompProfile/localhostProfile") != SECCOMP_PROFILE
            {
                failures.push(format!("container {name} does not select the Localhost seccomp profile {SECCOMP_PROFILE}"));
            }
            if text(context, "/appArmorProfile/type") != "Localhost"
                || text(context, "/appArmorProfile/localhostProfile") != profile
            {
                failures.push(format!(
                    "container {name} does not select the Localhost AppArmor profile {profile}"
                ));
            }
        } else {
            if !added.is_empty() {
                failures.push(format!("container {name} adds capabilities {added:?}"));
            }
            if context["runAsNonRoot"] != Value::Bool(true)
                || context["runAsUser"] != json!(65532)
                || context["allowPrivilegeEscalation"] != Value::Bool(false)
            {
                failures.push(format!(
                    "container {name} is not non-root 65532 without privilege escalation"
                ));
            }
            let seccomp = match text(context, "/seccompProfile/type") {
                "" => pod_seccomp,
                own => own,
            };
            if seccomp != "RuntimeDefault" {
                failures.push(format!(
                    "container {name} runs without the runtime's default seccomp profile"
                ));
            }
        }
    }
    failures
}

/// Design 33 § 6: `--apparmor-profile <profile>` appears exactly once when `profile` is not the
/// default, and not at all when it is. The one-word `--apparmor-profile=<profile>`, which clap
/// reads the same way, counts as a statement of the flag.
fn profile_flag(words: &[&str], profile: &str) -> Vec<String> {
    let stated: Vec<Option<&str>> = words
        .iter()
        .enumerate()
        .filter_map(|(at, word)| {
            if *word == PROFILE_FLAG {
                Some(words.get(at + 1).copied())
            } else {
                one_word_value(word, PROFILE_FLAG).map(Some)
            }
        })
        .collect();
    let expected: &[Option<&str>] = if profile == APPARMOR_PROFILE {
        &[]
    } else {
        &[Some(profile)]
    };
    if stated == expected {
        Vec::new()
    } else {
        vec![format!(
            "{PROFILE_FLAG} is stated as {stated:?} in {words:?}, not {expected:?} for the AppArmor profile {profile}"
        )]
    }
}

/// The bootstrap states a named profile before `--`, and nothing after it passes one to the
/// daemon.
fn bootstrap_profile(args: &[Value], profile: &str) -> Vec<String> {
    let words: Vec<&str> = args.iter().filter_map(Value::as_str).collect();
    let separator = words
        .iter()
        .position(|word| *word == "--")
        .unwrap_or(words.len());
    let mut failures: Vec<String> = profile_flag(&words[..separator], profile)
        .into_iter()
        .map(|failure| format!("the bootstrap: {failure}"))
        .collect();
    if states_flag(&words[separator..], PROFILE_FLAG) {
        failures.push(format!("the daemon itself is passed {PROFILE_FLAG}"));
    }
    failures
}

/// The value of `word` when it is `flag` in clap's one-word spelling, `--flag=<value>`.
fn one_word_value<'a>(word: &'a str, flag: &str) -> Option<&'a str> {
    word.strip_prefix(flag)?.strip_prefix('=')
}

/// Whether `words` state `flag` in either spelling clap reads: `--flag <value>` or
/// `--flag=<value>`.
fn states_flag(words: &[&str], flag: &str) -> bool {
    words
        .iter()
        .any(|word| *word == flag || one_word_value(word, flag).is_some())
}

/// The hosted daemon never listens on development TCP, maps a peer uid or takes a cgroup root.
fn forbidden_daemon_flags(words: &[&str]) -> Vec<String> {
    words
        .iter()
        .filter(|word| {
            word.starts_with("--tcp-")
                || states_flag(&[word], "--allow-uid")
                || word.starts_with("--cgroup-root")
        })
        .map(|word| format!("the daemon is started with {word}"))
        .collect()
}

fn pairs(args: &[Value]) -> Vec<(&str, &str)> {
    args.windows(2)
        .filter_map(|window| Some((window[0].as_str()?, window[1].as_str()?)))
        .collect()
}

#[allow(clippy::too_many_lines)] // One pass over the daemon container, in the order its spec reads.
fn check_daemon(pod: &Value, expect: &Expect) -> Vec<String> {
    let mut failures = Vec::new();
    let Some(daemon) = array(pod, "/containers")
        .iter()
        .find(|container| text(container, "/name") == "daemon")
    else {
        return vec!["the pod has no daemon container".into()];
    };
    let command: Vec<&str> = array(daemon, "/command")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let args = array(daemon, "/args");
    let words: Vec<&str> = args.iter().filter_map(Value::as_str).collect();
    let flags = pairs(args);
    let value = |flag: &str| {
        flags
            .iter()
            .find(|(name, _)| *name == flag)
            .map(|(_, value)| *value)
    };
    let entrypoint = if expect.execution {
        "/usr/local/bin/substrate-container-exec"
    } else {
        "/usr/local/bin/substrate-daemon"
    };
    if command != [entrypoint] {
        failures.push(format!("the daemon starts {command:?}, not {entrypoint}"));
    }
    if expect.execution != words.contains(&"--") {
        failures.push(
            "the bootstrap separator `--` is present exactly when execution is enabled: violated"
                .into(),
        );
    }
    if expect.execution {
        failures.extend(bootstrap_profile(args, &expect.apparmor_profile));
    } else if states_flag(&words, PROFILE_FLAG) {
        failures.push(format!(
            "the daemon is started with {PROFILE_FLAG} without the execution bootstrap"
        ));
    }
    if expect.quotas != (words.first() == Some(&"--project-quotas"))
        || expect.quotas != value("--project-quota-ids").is_some()
    {
        failures.push("the quota executable and its identity range are selected exactly together with project quotas: violated".into());
    }
    if value("--state") != Some(STATE_PATH) {
        failures.push(format!(
            "--state is {:?}, not {STATE_PATH}",
            value("--state")
        ));
    }
    if !array(daemon, "/volumeMounts")
        .iter()
        .any(|mount| text(mount, "/name") == "ledger" && text(mount, "/mountPath") == STATE_MOUNT)
    {
        failures.push(format!("the ledger claim is not mounted at {STATE_MOUNT}"));
    }
    if value("--deployment") != Some("$(POD_NAME)")
        || !array(daemon, "/env").iter().any(|variable| {
            text(variable, "/name") == "POD_NAME"
                && text(variable, "/valueFrom/fieldRef/fieldPath") == "metadata.name"
        })
    {
        failures.push("--deployment is not the pod's own stable name".into());
    }
    for required in [
        "--tls-listen",
        "--tls-certificate-chain",
        "--tls-private-key",
        "--hosted-identity-origin",
        "--hosted-identity-ca-bundle",
        "--ops-listen",
    ] {
        if value(required).is_none_or(str::is_empty) {
            failures.push(format!("the daemon is started without {required}"));
        }
    }
    failures.extend(forbidden_daemon_flags(&words));
    let operations = value("--ops-listen").unwrap_or("");
    if !operations.ends_with(":9090")
        || !array(daemon, "/ports")
            .iter()
            .any(|port| text(port, "/name") == "operations" && port["containerPort"] == json!(9090))
    {
        failures
            .push("the operational listener is not the container's operations port 9090".into());
    }
    for (probe, path) in [
        ("startupProbe", "/readyz"),
        ("readinessProbe", "/readyz"),
        ("livenessProbe", "/livez"),
    ] {
        if text(daemon, &format!("/{probe}/httpGet/path")) != path
            || text(daemon, &format!("/{probe}/httpGet/port")) != "operations"
        {
            failures.push(format!(
                "{probe} is not an httpGet of {path} on the operations port"
            ));
        }
    }
    for volume in ["tls", "identity"] {
        if !array(daemon, "/volumeMounts")
            .iter()
            .any(|mount| text(mount, "/name") == volume && mount["readOnly"] == Value::Bool(true))
        {
            failures.push(format!(
                "the daemon does not mount the {volume} copy read-only"
            ));
        }
    }
    failures
}

/// K3 A: two init containers copy the projected material into memory-backed `emptyDir`s.
fn check_init(pod: &Value) -> Vec<String> {
    let mut failures = Vec::new();
    let volumes = array(pod, "/volumes");
    let volume = |name: &str| volumes.iter().find(|volume| text(volume, "/name") == name);
    for (init, source, destination, key) in [
        ("tls-material", "tls-source", "tls", "secret"),
        (
            "identity-trust-roots",
            "identity-source",
            "identity",
            "configMap",
        ),
    ] {
        let Some(container) = array(pod, "/initContainers")
            .iter()
            .find(|container| text(container, "/name") == init)
        else {
            failures.push(format!("no {init} init container"));
            continue;
        };
        let command: Vec<&str> = array(container, "/command")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if command.first() != Some(&"/usr/bin/install") || !command.contains(&"0400") {
            failures.push(format!("{init} does not install its files with mode 0400"));
        }
        if volume(source).is_none_or(|volume| volume[key]["defaultMode"] != json!(0o440)) {
            failures.push(format!(
                "{source} is not a {key} volume with defaultMode 0440"
            ));
        }
        if volume(destination).is_none_or(|volume| text(volume, "/emptyDir/medium") != "Memory") {
            failures.push(format!("{destination} is not a memory-backed emptyDir"));
        }
    }
    failures
}

fn check_services(documents: &[Value], statefulset: &Value, labels: &Value) -> Vec<String> {
    let mut failures = Vec::new();
    let governing = text(statefulset, "/spec/serviceName");
    let mut selecting = 0;
    for service in documents
        .iter()
        .filter(|document| kind(document) == "Service")
    {
        let name = text(service, "/metadata/name");
        if matches!(
            text(service, "/spec/type"),
            "LoadBalancer" | "NodePort" | "ExternalName"
        ) {
            failures.push(format!(
                "Service {name} is a {}",
                text(service, "/spec/type")
            ));
        }
        if may_select_pods(&service["spec"]["selector"], labels) {
            selecting += 1;
            if text(service, "/spec/clusterIP") != "None" || name != governing {
                failures.push(format!(
                    "Service {name} selects the daemon pods but is not the headless governing Service {governing}"
                ));
            }
            if service["spec"]["publishNotReadyAddresses"] == Value::Bool(true) {
                failures.push(format!(
                    "Service {name} publishes pods before they are ready"
                ));
            }
        }
    }
    if selecting != 1 {
        failures.push(format!(
            "{selecting} Services select the daemon pods; exactly the headless one may"
        ));
    }
    failures
}

fn check_network_policy(documents: &[Value], labels: &Value, expect: &Expect) -> Vec<String> {
    let mut failures = Vec::new();
    let policies: Vec<&Value> = documents
        .iter()
        .filter(|document| kind(document) == "NetworkPolicy")
        .collect();
    let [policy] = policies.as_slice() else {
        return vec![format!(
            "renders {} NetworkPolicies, not one",
            policies.len()
        )];
    };
    let spec = &policy["spec"];
    if !selects(&spec["podSelector"]["matchLabels"], labels) {
        failures.push("the NetworkPolicy does not select the daemon pods".into());
    }
    let types = array(spec, "/policyTypes");
    if !types.contains(&json!("Ingress")) || !types.contains(&json!("Egress")) {
        failures.push("the NetworkPolicy does not govern both ingress and egress".into());
    }
    for rule in array(spec, "/ingress") {
        let peers = array(rule, "/from");
        let ports = array(rule, "/ports");
        if peers.is_empty() || ports.is_empty() {
            failures.push("an ingress rule admits every source or every port".into());
        }
        for peer in peers {
            if peer.get("ipBlock").is_some() {
                failures.push(format!(
                    "an ingress rule admits the address range {peer}; the chart admits clients by selector only"
                ));
            } else if let Some(open) = open_peer(peer) {
                failures.push(format!("an ingress rule admits {open}"));
            }
        }
        for port in ports {
            match port_number(port) {
                None => failures.push(format!("an ingress rule admits every port: {port}")),
                Some(8443 | 9090) => {}
                Some(other) => failures.push(format!(
                    "an ingress rule opens port {other}, neither the control port 8443 nor the operations port 9090"
                )),
            }
        }
    }
    let mut destinations = BTreeSet::new();
    for rule in array(spec, "/egress") {
        let peers = array(rule, "/to");
        let ports = array(rule, "/ports");
        if peers.is_empty() || ports.is_empty() {
            failures.push("an egress rule admits every destination or every port".into());
        }
        let numbers: Vec<Option<u64>> = ports.iter().map(port_number).collect();
        if numbers.contains(&None) {
            failures.push(format!(
                "an egress rule admits every port: {}",
                Value::Array(ports.to_vec())
            ));
        }
        for peer in peers {
            if let Some(cidr) = peer.pointer("/ipBlock/cidr").and_then(Value::as_str) {
                if admits_every_address(cidr) && !expect.allow_any_egress {
                    failures.push(format!("an egress rule admits {cidr}, every address"));
                }
                destinations.extend(
                    numbers
                        .iter()
                        .flatten()
                        .map(|port| (cidr.to_owned(), *port)),
                );
            } else if let Some(open) = open_peer(peer) {
                failures.push(format!("an egress rule admits {open}"));
            } else if numbers.iter().any(|port| *port != Some(53)) {
                failures.push(format!(
                    "an egress rule to the selected peer {peer} opens a port other than DNS"
                ));
            }
        }
    }
    if let Some(declared) = &expect.egress
        && destinations != *declared
    {
        failures.push(format!(
            "the egress rules admit {destinations:?}, not the declared destinations {declared:?}"
        ));
    }
    failures
}

/// What a selector peer admits beyond a named set, or `None` when every selector it carries names
/// labels: an empty peer is every peer, and an empty selector every pod or every namespace.
fn open_peer(peer: &Value) -> Option<String> {
    let Some(object) = peer.as_object() else {
        return Some(format!("the malformed peer {peer}"));
    };
    if object.is_empty() {
        return Some("every peer ({})".into());
    }
    for (key, what) in [
        ("namespaceSelector", "every namespace"),
        ("podSelector", "every pod"),
    ] {
        if let Some(selector) = object.get(key) {
            let named = |field: &str| {
                selector.get(field).is_some_and(|value| match value {
                    Value::Object(map) => !map.is_empty(),
                    Value::Array(items) => !items.is_empty(),
                    _ => false,
                })
            };
            if !named("matchLabels") && !named("matchExpressions") {
                return Some(format!("{what} through the empty {key} of {peer}"));
            }
        }
    }
    if !object.contains_key("namespaceSelector") && !object.contains_key("podSelector") {
        return Some(format!("the peer {peer}, which names no selector"));
    }
    None
}

/// The one port a `NetworkPolicyPort` admits, or `None` when it admits a range or every port.
fn port_number(port: &Value) -> Option<u64> {
    if port.get("endPort").is_some() {
        return None;
    }
    port.get("port").and_then(Value::as_u64)
}

fn check_certificate_names(
    documents: &[Value],
    statefulset: &Value,
    expect: &Expect,
) -> Vec<String> {
    let mut failures = Vec::new();
    let service = text(statefulset, "/spec/serviceName");
    let name = text(statefulset, "/metadata/name");
    let Some(names) = documents
        .iter()
        .find(|document| {
            kind(document) == "ConfigMap"
                && text(document, "/metadata/name") == format!("{name}-certificate-names")
        })
        .map(|document| text(document, "/data/dns-names"))
    else {
        return vec!["no certificate-names ConfigMap".into()];
    };
    let listed: Vec<&str> = names.lines().collect();
    let wanted: Vec<String> = (0..expect.replicas)
        .map(|ordinal| {
            format!(
                "{name}-{ordinal}.{service}.{}.svc.{}",
                expect.namespace, expect.domain
            )
        })
        .collect();
    if listed != wanted {
        failures.push(format!(
            "the certificate names are {listed:?}, not {wanted:?}"
        ));
    }
    let shared = [
        service.to_owned(),
        format!("{service}.{}", expect.namespace),
        format!("{service}.{}.svc", expect.namespace),
        format!("{service}.{}.svc.{}", expect.namespace, expect.domain),
    ];
    for listed in &listed {
        if listed.contains('*') || shared.iter().any(|shared| shared == listed) {
            failures.push(format!(
                "certificate name {listed} resolves to more than one pod"
            ));
        }
    }
    failures
}

fn check_node_profiles(documents: &[Value], expect: &Expect) -> Vec<String> {
    let mut failures = Vec::new();
    let daemonsets: Vec<&Value> = documents
        .iter()
        .filter(|document| kind(document) == "DaemonSet")
        .collect();
    let Some(namespace) = &expect.node_profiles else {
        if !daemonsets.is_empty() {
            failures.push("renders a node-profile DaemonSet that was not enabled".into());
        }
        return failures;
    };
    let [daemonset] = daemonsets.as_slice() else {
        return vec![format!("renders {} DaemonSets, not one", daemonsets.len())];
    };
    if text(daemonset, "/metadata/namespace") != namespace {
        failures.push("the node-profile DaemonSet is not in its own namespace".into());
    }
    let pod = &daemonset["spec"]["template"]["spec"];
    if pod["automountServiceAccountToken"] != Value::Bool(false) {
        failures.push("the node-profile installer mounts a ServiceAccount token".into());
    }
    for field in ["hostNetwork", "hostPID", "hostIPC"] {
        if pod[field] == Value::Bool(true) {
            failures.push(format!("the node-profile installer sets {field}"));
        }
    }
    let paths: BTreeSet<&str> = array(pod, "/volumes")
        .iter()
        .filter_map(|volume| volume.pointer("/hostPath/path").and_then(Value::as_str))
        .collect();
    if paths
        != BTreeSet::from([
            "/sys/kernel/security",
            "/var/lib/kubelet/seccomp/b10x-substrate",
        ])
    {
        failures.push(format!(
            "the node-profile installer mounts host paths {paths:?}, not exactly securityfs and its seccomp directory"
        ));
    }
    for container in array(pod, "/containers") {
        for (what, pointer) in [
            ("command", "/command"),
            ("readiness probe", "/readinessProbe/exec/command"),
        ] {
            let words: Vec<&str> = array(container, pointer)
                .iter()
                .filter_map(Value::as_str)
                .collect();
            failures.extend(
                profile_flag(&words, &expect.apparmor_profile)
                    .into_iter()
                    .map(|failure| format!("the node-profile installer's {what}: {failure}")),
            );
        }
        let context = &container["securityContext"];
        let added: Vec<&str> = array(context, "/capabilities/add")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if context["privileged"] == Value::Bool(true)
            || added != ["MAC_ADMIN"]
            || !array(context, "/capabilities/drop").contains(&json!("ALL"))
        {
            failures.push("the node-profile installer holds more than MAC_ADMIN".into());
        }
        if context["readOnlyRootFilesystem"] != Value::Bool(true) {
            failures.push(
                "the node-profile installer does not set readOnlyRootFilesystem: true".into(),
            );
        }
    }
    failures.extend(pool_binding(pod, &expect.pool, "node-profile installer"));
    failures.extend(pod_annotations(
        &daemonset["spec"]["template"]["metadata"],
        "node-profile installer",
    ));
    failures
}

#[derive(Debug, ClapArgs)]
pub struct PackageArgs {
    /// The release version: the chart's version and appVersion.
    #[arg(long)]
    version: String,
    /// The published daemon image digest the chart's default values pin.
    #[arg(long)]
    image_digest: String,
    /// Directory the packaged chart archive is written to.
    #[arg(long)]
    out: PathBuf,
    /// The `helm` executable.
    #[arg(long, default_value = "helm")]
    helm: PathBuf,
}

/// The source chart's `Chart.yaml` and `values.yaml` with the release version and image digest.
pub fn released_sources(
    chart_yaml: &str,
    values_yaml: &str,
    version: &str,
    digest: &str,
) -> Result<(String, String)> {
    ensure!(
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "version {version:?} is not the bare MAJOR.MINOR.PATCH form"
    );
    ensure!(
        digest.len() == 71
            && digest.starts_with("sha256:")
            && digest[7..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "image digest {digest:?} is not sha256:<64 lowercase hex>"
    );
    let replace_once = |text: &str, from: &str, to: &str, file: &str| -> Result<String> {
        let count = text.lines().filter(|line| *line == from).count();
        ensure!(
            count == 1,
            "{file} carries {count} lines {from:?}; the packager rewrites exactly one"
        );
        let mut result = String::new();
        for line in text.lines() {
            let line = if line == from { to } else { line };
            writeln!(result, "{line}").expect("writing to a String cannot fail");
        }
        Ok(result)
    };
    let chart = replace_once(
        chart_yaml,
        "version: 0.0.0",
        &format!("version: {version}"),
        "Chart.yaml",
    )?;
    let chart = replace_once(
        &chart,
        "appVersion: \"0.0.0\"",
        &format!("appVersion: \"{version}\""),
        "Chart.yaml",
    )?;
    let values = replace_once(
        values_yaml,
        "  digest: \"\"",
        &format!("  digest: \"{digest}\""),
        "values.yaml",
    )?;
    Ok((chart, values))
}

/// Every regular file beneath `directory`, by POSIX path relative to it; anything else is refused.
fn chart_files(directory: &Path, prefix: &str, files: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| anyhow::anyhow!("{} is not a UTF-8 file name", name.display()))?;
        let path = format!("{prefix}{name}");
        if kind.is_dir() {
            chart_files(&entry.path(), &format!("{path}/"), files)?;
        } else if kind.is_file() {
            files.push((path, fs::read(entry.path())?));
        } else {
            bail!(
                "{} is neither a file nor a directory",
                entry.path().display()
            );
        }
    }
    Ok(())
}

/// The released chart archive, byte for byte a function of the source chart, the version and the
/// digest: files in path order under `b10x-substrate/`, each mode 0644, owner 0, modification time
/// 0, in a ustar stream gzipped with stored blocks and no name or time in its header. `helm
/// package` stamps every entry with the time it runs, so two packagings of one commit differ and
/// a release recovery could not compare the chart it would push with the one already published.
pub fn package_archive(source: &Path, version: &str, digest: &str) -> Result<Vec<u8>> {
    let (chart, values) = released_sources(
        &fs::read_to_string(source.join("Chart.yaml"))?,
        &fs::read_to_string(source.join("values.yaml"))?,
        version,
        digest,
    )?;
    let mut files = Vec::new();
    chart_files(source, "", &mut files)?;
    for (path, bytes) in &mut files {
        match path.as_str() {
            "Chart.yaml" => *bytes = chart.as_bytes().to_vec(),
            "values.yaml" => *bytes = values.as_bytes().to_vec(),
            _ => {}
        }
    }
    files.sort();
    let mut tar = Vec::new();
    for (path, bytes) in &files {
        tar.extend_from_slice(&ustar_header(
            &format!("b10x-substrate/{path}"),
            bytes.len(),
        )?);
        tar.extend_from_slice(bytes);
        tar.resize(tar.len().next_multiple_of(512), 0);
    }
    tar.resize(tar.len() + 1024, 0);
    gzip_stored(&tar)
}

fn ustar_header(name: &str, size: usize) -> Result<[u8; 512]> {
    ensure!(
        name.len() <= 100,
        "{name} is longer than a ustar name field"
    );
    let mut header = [0_u8; 512];
    let mut put = |offset: usize, value: &[u8]| {
        header[offset..offset + value.len()].copy_from_slice(value);
    };
    put(0, name.as_bytes());
    put(100, b"0000644\0");
    put(108, b"0000000\0");
    put(116, b"0000000\0");
    put(124, format!("{size:011o}\0").as_bytes());
    put(136, b"00000000000\0");
    put(148, b"        ");
    put(156, b"0");
    put(257, b"ustar\0");
    put(263, b"00");
    let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(header)
}

/// RFC 1952 gzip of `data` in RFC 1951 stored blocks: no compression, so no compressor version
/// can change a byte.
fn gzip_stored(data: &[u8]) -> Result<Vec<u8>> {
    let length = u32::try_from(data.len()).context("the chart archive exceeds 4 GiB")?;
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut chunks = data.chunks(usize::from(u16::MAX)).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none()));
        let size = u16::try_from(chunk.len()).expect("chunks are at most u16::MAX bytes");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(!size).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&length.to_le_bytes());
    Ok(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub fn run_package(args: &PackageArgs) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let bytes = package_archive(&root.join(CHART), &args.version, &args.image_digest)?;
    fs::create_dir_all(&args.out)?;
    let archive = args
        .out
        .join(format!("b10x-substrate-{}.tgz", args.version));
    fs::write(&archive, &bytes)?;

    let mut values = base_values();
    values["image"] = json!({});
    let rendered = match render(&args.helm, &archive, RELEASE, NAMESPACE, &values)? {
        Ok(text) => text,
        Err(stderr) => bail!("the packaged chart does not render with its own defaults: {stderr}"),
    };
    let image = format!(
        "ghcr.io/beyond10x/b10x-substrate-daemon@{}",
        args.image_digest
    );
    let documents = chart_yaml::documents(&rendered)?;
    let images: BTreeSet<&str> = documents
        .iter()
        .filter(|document| kind(document) == "StatefulSet")
        .flat_map(|statefulset| {
            array(statefulset, "/spec/template/spec/containers")
                .iter()
                .chain(array(statefulset, "/spec/template/spec/initContainers"))
        })
        .map(|container| text(container, "/image"))
        .collect();
    ensure!(
        images == BTreeSet::from([image.as_str()]),
        "the packaged chart's default images are {images:?}, not {image}"
    );
    println!("{}", archive.display());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Value, json};

    use super::{
        Expect, Posture, base_values, bootstrap_profile, check_certificate_names, check_daemon,
        check_network_policy, check_services, forbidden_daemon_flags, merged, pod_posture,
        profile_flag, refused_cases, released_sources,
    };
    use crate::chart_yaml;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../deploy/helm/substrate");

    /// Every value a template writes into YAML is quoted, an `int`, or structured output
    /// (`toYaml`, an indented include), so a value rendered with the schema skipped stays one
    /// scalar and cannot add a key, a list item or a document.
    #[test]
    fn every_template_interpolation_is_quoted_an_int_or_structured() {
        let structured = [
            "toYaml",
            "nindent",
            "include \"substrate.selectorLabels\" .",
        ];
        let mut unquoted = Vec::new();
        let directory = std::path::Path::new(ROOT).join("templates");
        let mut names: Vec<_> = std::fs::read_dir(&directory)
            .expect("templates")
            .map(|entry| entry.expect("template").path())
            .filter(|path| path.file_name().is_some_and(|name| name != "NOTES.txt"))
            .collect();
        names.sort();
        for path in names {
            let text = std::fs::read_to_string(&path).expect("template text");
            for (number, line) in text.lines().enumerate() {
                let mut rest = line;
                while let Some(start) = rest.find("{{ ") {
                    let end = rest[start..]
                        .find("}}")
                        .map_or(rest.len(), |end| start + end);
                    let expression = &rest[start + 3..end];
                    if !(expression.contains("| quote")
                        || expression.starts_with("int ")
                        || structured.iter().any(|form| expression.contains(form)))
                    {
                        unquoted.push(format!(
                            "{}:{}: {}",
                            path.file_name().unwrap().to_string_lossy(),
                            number + 1,
                            line.trim()
                        ));
                    }
                    rest = &rest[end..];
                }
            }
        }
        assert!(
            unquoted.is_empty(),
            "unquoted interpolations: {unquoted:#?}"
        );
    }

    fn schema() -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(format!("{ROOT}/values.schema.json")).unwrap(),
        )
        .unwrap()
    }

    fn defaults() -> Value {
        chart_yaml::documents(&std::fs::read_to_string(format!("{ROOT}/values.yaml")).unwrap())
            .unwrap()
            .remove(0)
    }

    #[test]
    fn the_values_schema_is_a_closed_draft_2020_12_schema() {
        fn closed(value: &Value, at: &str) {
            if let Some(object) = value.as_object() {
                if object.get("type") == Some(&json!("object")) {
                    let closed = object.get("additionalProperties") == Some(&json!(false));
                    let bounded_map = object
                        .get("additionalProperties")
                        .is_some_and(Value::is_object)
                        && object.contains_key("propertyNames");
                    assert!(
                        closed || bounded_map || at == "/properties/global",
                        "{at} is an open object"
                    );
                }
                for (key, child) in object {
                    closed(child, &format!("{at}/{key}"));
                }
            }
        }
        let schema = schema();
        jsonschema::draft202012::meta::validate(&schema).expect("meta-schema validation");
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["additionalProperties"], false);
        closed(&schema, "");
    }

    #[test]
    fn the_defaults_alone_are_refused_and_the_lane_values_complete_them() {
        let validator = jsonschema::validator_for(&schema()).unwrap();
        assert!(
            !validator.is_valid(&defaults()),
            "the chart's defaults name no pool, certificate or Identity and must not install"
        );
        let lane = merged(&defaults(), &base_values());
        let errors: Vec<String> = validator
            .iter_errors(&lane)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn every_schema_refusal_is_refused_offline_and_every_posture_class_is_covered() {
        let validator = jsonschema::validator_for(&schema()).unwrap();
        let lane = merged(&defaults(), &base_values());
        let mut postures = BTreeSet::new();
        for case in refused_cases() {
            postures.insert(case.posture);
            if case.message.is_none() {
                assert!(
                    !validator.is_valid(&merged(&lane, &case.values)),
                    "the schema admits {}",
                    case.name
                );
            }
        }
        for posture in [
            Posture::Privilege,
            Posture::HostNamespace,
            Posture::HostPath,
            Posture::Cgroup,
            Posture::Mount,
            Posture::Quota,
            Posture::Transport,
            Posture::Egress,
        ] {
            assert!(
                postures.contains(&posture),
                "no refusal case for {posture:?}"
            );
        }
    }

    fn container(name: &str, context: &Value) -> Value {
        json!({"name": name, "securityContext": context})
    }

    fn restricted() -> Value {
        json!({"runAsNonRoot": true, "runAsUser": 65532, "allowPrivilegeEscalation": false,
               "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"}, "readOnlyRootFilesystem": true})
    }

    #[test]
    fn the_pod_posture_check_refuses_privilege_host_namespaces_and_host_paths() {
        let clean = json!({"containers": [container("daemon", &restricted())], "volumes": [{"name": "tmp", "emptyDir": {}}]});
        assert!(pod_posture(&clean, None, "pod").is_empty());

        let mut privileged = restricted();
        privileged["privileged"] = json!(true);
        let pod = json!({"containers": [container("daemon", &privileged)]});
        assert!(
            pod_posture(&pod, None, "pod")
                .iter()
                .any(|f| f.contains("privileged"))
        );

        let pod = json!({"hostPID": true, "containers": [container("daemon", &restricted())]});
        assert!(
            pod_posture(&pod, None, "pod")
                .iter()
                .any(|f| f.contains("hostPID"))
        );

        let pod = json!({"containers": [container("daemon", &restricted())], "volumes": [{"name": "root", "hostPath": {"path": "/"}}]});
        assert!(
            pod_posture(&pod, None, "pod")
                .iter()
                .any(|f| f.contains("host path"))
        );

        let mut extra = json!({"capabilities": {"drop": ["ALL"], "add": ["CHOWN", "SETUID", "SETGID", "SETPCAP", "SYS_ADMIN", "NET_ADMIN"]},
            "seccompProfile": {"type": "Localhost", "localhostProfile": "b10x-substrate/host-exec-v1-amd64.json"},
            "appArmorProfile": {"type": "Localhost", "localhostProfile": "substrate-host-exec-v2"}, "readOnlyRootFilesystem": true});
        let pod = json!({"containers": [container("daemon", &extra)]});
        assert!(
            pod_posture(&pod, Some(super::APPARMOR_PROFILE), "pod")
                .iter()
                .any(|f| f.contains("five"))
        );
        extra["capabilities"]["add"] = json!(["CHOWN", "SETUID", "SETGID", "SETPCAP", "SYS_ADMIN"]);
        let pod = json!({"containers": [container("daemon", &extra)]});
        assert!(
            pod_posture(&pod, Some(super::APPARMOR_PROFILE), "pod").is_empty(),
            "{:?}",
            pod_posture(&pod, Some(super::APPARMOR_PROFILE), "pod")
        );
        assert!(
            !pod_posture(&pod, None, "pod").is_empty(),
            "five capabilities without execution"
        );
    }

    #[test]
    fn a_second_service_selecting_the_pods_is_refused() {
        let labels = json!({"app": "d"});
        let statefulset = json!({"spec": {"serviceName": "d"}});
        let headless = json!({"kind": "Service", "metadata": {"name": "d"}, "spec": {"clusterIP": "None", "selector": {"app": "d"}}});
        assert!(check_services(std::slice::from_ref(&headless), &statefulset, &labels).is_empty());
        let round_robin = json!({"kind": "Service", "metadata": {"name": "d-api"}, "spec": {"selector": {"app": "d"}}});
        let failures = check_services(&[headless, round_robin], &statefulset, &labels);
        assert!(failures.iter().any(|f| f.contains("d-api")), "{failures:?}");
    }

    #[test]
    fn certificate_names_that_reach_the_service_are_refused() {
        let statefulset = json!({"metadata": {"name": "d"}, "spec": {"serviceName": "d"}});
        let expect = Expect {
            replicas: 1,
            namespace: "n".into(),
            ..Expect::lane()
        };
        let configmap = |names: &str| json!({"kind": "ConfigMap", "metadata": {"name": "d-certificate-names"}, "data": {"dns-names": names}});
        assert!(
            check_certificate_names(
                &[configmap("d-0.d.n.svc.cluster.local")],
                &statefulset,
                &expect
            )
            .is_empty()
        );
        for names in [
            "d-0.d.n.svc.cluster.local\nd.n.svc.cluster.local",
            "*.d.n.svc.cluster.local",
        ] {
            assert!(
                !check_certificate_names(&[configmap(names)], &statefulset, &expect).is_empty(),
                "{names}"
            );
        }
    }

    /// clap reads `--flag=value` as `--flag value`, so every flag the lane refuses by name is
    /// refused in its one-word spelling too.
    #[test]
    fn a_refused_daemon_flag_in_its_one_word_spelling_is_refused() {
        let daemon = |args: &[&str]| {
            json!({"containers": [{
                "name": "daemon",
                "command": ["/usr/local/bin/substrate-daemon"],
                "args": args,
            }]})
        };
        for word in [
            "--apparmor-profile=substrate-host-exec-v1",
            "--apparmor-profile=",
        ] {
            let failures = check_daemon(&daemon(&[word]), &Expect::lane());
            assert!(
                failures
                    .iter()
                    .any(|failure| failure.contains("without the execution bootstrap")),
                "{word} admitted without the bootstrap: {failures:?}"
            );
        }
        assert!(
            !check_daemon(&daemon(&[]), &Expect::lane())
                .iter()
                .any(|failure| failure.contains("without the execution bootstrap"))
        );
        for words in [
            &["--allow-uid", "0"][..],
            &["--allow-uid=0"],
            &["--allow-uid="],
            &["--tcp-listen=127.0.0.1:1"],
            &["--cgroup-root=/sys/fs/cgroup"],
        ] {
            assert!(
                !forbidden_daemon_flags(words).is_empty(),
                "{words:?} admitted"
            );
        }
        assert!(forbidden_daemon_flags(&["--allow-uids-x", "--state", "/s"]).is_empty());
    }

    #[test]
    fn an_open_network_policy_rule_is_refused() {
        let labels = json!({"app": "d"});
        let policy = |spec: Value| json!({"kind": "NetworkPolicy", "spec": spec});
        let closed = json!({"podSelector": {"matchLabels": {"app": "d"}}, "policyTypes": ["Ingress", "Egress"],
            "egress": [{"to": [{"ipBlock": {"cidr": "192.0.2.1/32"}}], "ports": [{"port": 443}]}]});
        assert!(
            check_network_policy(&[policy(closed.clone())], &labels, &Expect::lane()).is_empty()
        );
        let mut open = closed.clone();
        open["ingress"] = json!([{"ports": [{"port": 8443}]}]);
        assert!(!check_network_policy(&[policy(open)], &labels, &Expect::lane()).is_empty());
        let mut any = closed;
        any["egress"][0]["to"][0]["ipBlock"]["cidr"] = json!("0.0.0.0/0");
        assert!(!check_network_policy(&[policy(any.clone())], &labels, &Expect::lane()).is_empty());
        let admitted = Expect {
            allow_any_egress: true,
            ..Expect::lane()
        };
        assert!(check_network_policy(&[policy(any)], &labels, &admitted).is_empty());
    }

    #[test]
    fn the_packager_sets_the_version_and_pins_the_digest_exactly_once() {
        let chart = std::fs::read_to_string(format!("{ROOT}/Chart.yaml")).unwrap();
        let values = std::fs::read_to_string(format!("{ROOT}/values.yaml")).unwrap();
        let digest = format!("sha256:{}", "a".repeat(64));
        let (chart_out, values_out) = released_sources(&chart, &values, "1.2.3", &digest).unwrap();
        assert!(chart_out.lines().any(|line| line == "version: 1.2.3"));
        assert!(
            chart_out
                .lines()
                .any(|line| line == "appVersion: \"1.2.3\"")
        );
        assert!(
            values_out
                .lines()
                .any(|line| line == format!("  digest: \"{digest}\""))
        );
        assert_eq!(
            chart_yaml::documents(&values_out).unwrap()[0]["image"]["digest"],
            json!(digest)
        );
        for (version, digest) in [
            ("1.2.3-rc.0", digest.as_str()),
            ("1.2.3", "sha256:abc"),
            ("v1.2.3", digest.as_str()),
        ] {
            assert!(
                released_sources(&chart, &values, version, digest).is_err(),
                "{version} {digest}"
            );
        }
        assert!(
            released_sources(&chart_out, &values, "1.2.3", &digest).is_err(),
            "a released Chart.yaml is not a source"
        );
    }

    #[test]
    fn security_review_the_pod_posture_check_refuses_a_writable_root_filesystem() {
        let mut writable = restricted();
        writable["readOnlyRootFilesystem"] = json!(false);
        let pod = json!({"containers": [container("daemon", &writable)]});
        assert!(
            !pod_posture(&pod, None, "pod").is_empty(),
            "a daemon container with a writable root filesystem passes the K4 posture check"
        );
        let bootstrap = json!({"capabilities": {"drop": ["ALL"], "add": super::BOOTSTRAP_CAPABILITIES},
            "seccompProfile": {"type": "Localhost", "localhostProfile": "b10x-substrate/host-exec-v1-amd64.json"},
            "appArmorProfile": {"type": "Localhost", "localhostProfile": "substrate-host-exec-v2"},
            "readOnlyRootFilesystem": false});
        let pod = json!({"containers": [container("daemon", &bootstrap)]});
        assert!(
            !pod_posture(&pod, Some(super::APPARMOR_PROFILE), "pod").is_empty(),
            "the execution bootstrap with a writable root filesystem passes the K4 posture check"
        );
    }

    #[test]
    fn security_review_an_egress_rule_admitting_every_address_in_any_spelling_is_refused() {
        let labels = json!({"app": "d"});
        for cidr in ["10.0.0.0/0", "1.2.3.4/0", "0::/0", "::0/0"] {
            let policy = json!({"kind": "NetworkPolicy", "spec": {
                "podSelector": {"matchLabels": {"app": "d"}},
                "policyTypes": ["Ingress", "Egress"],
                "egress": [{"to": [{"ipBlock": {"cidr": cidr}}], "ports": [{"port": 443}]}]}});
            assert!(
                !check_network_policy(&[policy], &labels, &Expect::lane()).is_empty(),
                "{cidr} admits every address and passes without allowAnyEgress"
            );
        }
    }

    /// S-F5: the packager is a function of its inputs, so a recovery can compare the chart it
    /// would push with the published one by digest. Two packagings of copies whose files carry
    /// different modification times and were written in different orders are byte-identical.
    #[test]
    fn the_packaged_chart_is_byte_identical_across_packagings() {
        let source = std::path::Path::new(ROOT);
        let digest = format!("sha256:{}", "a".repeat(64));
        let copy = |reverse: bool| {
            let directory = tempfile::tempdir().unwrap();
            let mut files = Vec::new();
            super::chart_files(source, "", &mut files).unwrap();
            if reverse {
                files.reverse();
            }
            for (path, bytes) in files {
                let target = directory.path().join(path);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(&target, bytes).unwrap();
                if reverse {
                    std::fs::File::options()
                        .write(true)
                        .open(&target)
                        .unwrap()
                        .set_modified(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap();
                }
            }
            directory
        };
        let first = copy(false);
        let second = copy(true);
        let one = super::package_archive(first.path(), "1.2.3", &digest).unwrap();
        let two = super::package_archive(second.path(), "1.2.3", &digest).unwrap();
        assert_eq!(one, two, "two packagings of one source differ");
        assert_ne!(
            one,
            super::package_archive(first.path(), "1.2.4", &digest).unwrap(),
            "the version is not in the archive"
        );
        assert_eq!(
            &one[..4],
            &[0x1f, 0x8b, 8, 0],
            "not a gzip stream without a name"
        );
        let tar_length = u32::from_le_bytes(one[one.len() - 4..].try_into().unwrap());
        assert_eq!(tar_length % 512, 0);
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }

    /// A-F2: the rendered `(cidr, port)` set is the declared one, and no peer or port is open.
    #[test]
    fn egress_beyond_the_declared_destinations_is_refused() {
        let labels = json!({"app": "d"});
        let values = merged(&defaults(), &base_values());
        let declared = super::declared_egress(&values);
        assert_eq!(
            declared,
            BTreeSet::from([
                ("192.0.2.10/32".to_owned(), 443),
                ("192.0.2.20/32".to_owned(), 443),
                ("192.0.2.30/32".to_owned(), 443),
            ])
        );
        let expect = Expect {
            egress: Some(declared),
            ..Expect::lane()
        };
        let rule = |cidr: &str, port: u64| json!({"to": [{"ipBlock": {"cidr": cidr}}], "ports": [{"protocol": "TCP", "port": port}]});
        let dns = json!({"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}],
            "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]});
        let policy = |egress: Value| json!({"kind": "NetworkPolicy", "spec": {"podSelector": {"matchLabels": labels}, "policyTypes": ["Ingress", "Egress"], "ingress": [], "egress": egress}});
        let exact = json!([
            dns,
            rule("192.0.2.10/32", 443),
            rule("192.0.2.20/32", 443),
            rule("192.0.2.30/32", 443)
        ]);
        let found = check_network_policy(&[policy(exact.clone())], &labels, &expect);
        assert!(found.is_empty(), "{found:?}");
        let mut extra = exact.clone();
        extra
            .as_array_mut()
            .unwrap()
            .push(rule("198.51.100.0/24", 22));
        assert!(!check_network_policy(&[policy(extra)], &labels, &expect).is_empty());
        let mut missing = exact.clone();
        missing.as_array_mut().unwrap().pop();
        assert!(!check_network_policy(&[policy(missing)], &labels, &expect).is_empty());
        let mut wide_dns = exact;
        wide_dns[0]["ports"] =
            json!([{"protocol": "TCP", "port": 53}, {"protocol": "TCP", "port": 443}]);
        assert!(!check_network_policy(&[policy(wide_dns)], &labels, &expect).is_empty());
    }

    /// A-F3: every toleration is the pool's taint, and the selector names the pool's label.
    #[test]
    fn a_pod_off_the_pool_or_tolerating_more_than_its_taint_is_refused() {
        let pool = super::Pool::of(&base_values());
        let bound = json!({"nodeSelector": {"b10x.dev/substrate-pool": "lane"},
            "tolerations": [{"key": "b10x.dev/substrate-pool", "operator": "Equal", "value": "lane", "effect": "NoSchedule"}]});
        assert!(super::pool_binding(&bound, &pool, "pod").is_empty());
        let mut other_value = bound.clone();
        other_value["nodeSelector"]["b10x.dev/substrate-pool"] = json!("other");
        let mut extra = bound.clone();
        extra["tolerations"]
            .as_array_mut()
            .unwrap()
            .push(json!({"key": "node.kubernetes.io/unschedulable", "operator": "Exists"}));
        let mut no_key = bound.clone();
        no_key["tolerations"] = json!([{"operator": "Exists"}]);
        for pod in [other_value, extra, no_key] {
            assert!(!super::pool_binding(&pod, &pool, "pod").is_empty(), "{pod}");
        }
    }

    /// A-F7: a selector on any label the controller adds to a pod reaches that pod.
    #[test]
    fn every_controller_label_counts_as_selecting_the_pods() {
        let labels = json!({"app": "d"});
        for selector in [
            json!({"statefulset.kubernetes.io/pod-name": "d-1"}),
            json!({"apps.kubernetes.io/pod-index": "0"}),
            json!({"controller-revision-hash": "d-5f7c"}),
            json!({"app": "d", "apps.kubernetes.io/pod-index": "1"}),
        ] {
            assert!(super::may_select_pods(&selector, &labels), "{selector}");
        }
        for selector in [
            json!({}),
            json!({"app": "other"}),
            json!({"app": "d", "tier": "x"}),
        ] {
            assert!(!super::may_select_pods(&selector, &labels), "{selector}");
        }
    }

    /// A-F4 and S-F7: the schema refuses every profile-selecting annotation key, and the lane's
    /// own predicate agrees with it.
    #[test]
    fn profile_selecting_annotations_are_refused_by_schema_and_lane() {
        let validator = jsonschema::validator_for(&schema()).unwrap();
        let lane = merged(&defaults(), &base_values());
        for key in [
            "container.apparmor.security.beta.kubernetes.io/daemon",
            "container.AppArmor.security.beta.kubernetes.io/daemon",
            "apparmor.example/x",
            "seccomp.security.alpha.kubernetes.io/pod",
            "container.seccomp.security.alpha.kubernetes.io/daemon",
        ] {
            let values = merged(&lane, &json!({"podAnnotations": {key: "unconfined"}}));
            assert!(!validator.is_valid(&values), "the schema admits {key}");
            assert!(super::security_profile_annotation(key), "{key}");
        }
        let values = merged(
            &lane,
            &json!({"podAnnotations": {"b10x.dev/lane-roll": "1"}}),
        );
        assert!(validator.is_valid(&values));
        assert!(!super::security_profile_annotation("b10x.dev/lane-roll"));
    }

    const NAMED: &str = "substrate-host-exec-v1-lane-a";

    /// Design 33 § 6: the value defaults to the `v2` policy (D180), and the schema admits only the
    /// `v1` and `v2` families.
    #[test]
    fn the_profile_value_defaults_to_today_and_the_schema_admits_only_its_family() {
        assert_eq!(super::APPARMOR_PROFILE, "substrate-host-exec-v2");
        assert_eq!(
            defaults()["execution"]["appArmorProfile"],
            json!(super::APPARMOR_PROFILE)
        );
        let validator = jsonschema::validator_for(&schema()).unwrap();
        let lane = merged(&defaults(), &base_values());
        let longest = format!("substrate-host-exec-v1-{}", "a".repeat(40));
        let longest_v2 = format!("substrate-host-exec-v2-{}", "a".repeat(40));
        for name in [
            super::APPARMOR_PROFILE,
            "substrate-host-exec-v1",
            NAMED,
            "substrate-host-exec-v2-lane-a",
            longest.as_str(),
            longest_v2.as_str(),
        ] {
            let values = merged(&lane, &json!({"execution": {"appArmorProfile": name}}));
            assert!(validator.is_valid(&values), "the schema refuses {name}");
        }
        let too_long = format!("{longest}a");
        let too_long_v2 = format!("{longest_v2}a");
        for name in [
            "unconfined",
            "substrate-host-exec-v0",
            "substrate-host-exec-v3",
            "substrate-host-exec-v12",
            "substrate-host-exec-v1-",
            "substrate-host-exec-v2-",
            "substrate-host-exec-v1-A",
            "substrate-host-exec-v1-a\n",
            too_long.as_str(),
            too_long_v2.as_str(),
        ] {
            let values = merged(&lane, &json!({"execution": {"appArmorProfile": name}}));
            assert!(!validator.is_valid(&values), "the schema admits {name:?}");
        }
        assert!(
            super::accepted_cases()
                .iter()
                .any(
                    |case| case.expect.apparmor_profile != super::APPARMOR_PROFILE
                        && case.expect.node_profiles.is_some()
                ),
            "no accepted rendering states a named profile with the installer"
        );
        assert!(
            super::injected_cases()
                .iter()
                .any(|(_, values)| values.pointer("/execution/appArmorProfile").is_some()),
            "no injected profile name"
        );
    }

    /// Design 33 § 6: the daemon selects the configured profile, and the bootstrap and the
    /// installer state it exactly when it is not the default.
    #[test]
    fn a_named_profile_is_selected_and_stated_where_it_is_used() {
        let context = |profile: &str| {
            json!({"capabilities": {"drop": ["ALL"], "add": ["CHOWN", "SETUID", "SETGID", "SETPCAP", "SYS_ADMIN"]},
                "seccompProfile": {"type": "Localhost", "localhostProfile": "b10x-substrate/host-exec-v1-amd64.json"},
                "appArmorProfile": {"type": "Localhost", "localhostProfile": profile}, "readOnlyRootFilesystem": true})
        };
        let pod = |profile: &str| json!({"containers": [container("daemon", &context(profile))]});
        assert!(pod_posture(&pod(NAMED), Some(NAMED), "pod").is_empty());
        assert!(
            pod_posture(
                &pod(super::APPARMOR_PROFILE),
                Some(super::APPARMOR_PROFILE),
                "pod"
            )
            .is_empty()
        );
        for (selected, configured) in [
            (super::APPARMOR_PROFILE, NAMED),
            (NAMED, super::APPARMOR_PROFILE),
        ] {
            assert!(
                pod_posture(&pod(selected), Some(configured), "pod")
                    .iter()
                    .any(|failure| failure.contains("AppArmor")),
                "{selected} admitted for {configured}"
            );
        }

        let default = super::APPARMOR_PROFILE;
        assert!(profile_flag(&["--hold"], default).is_empty());
        assert!(profile_flag(&["--hold", "--apparmor-profile", NAMED], NAMED).is_empty());
        for (words, profile) in [
            (vec!["--hold", "--apparmor-profile", default], default),
            (vec!["--hold"], NAMED),
            (
                vec![
                    "--hold",
                    "--apparmor-profile",
                    "substrate-host-exec-v1-lane-b",
                ],
                NAMED,
            ),
            (
                vec!["--apparmor-profile", NAMED, "--apparmor-profile", NAMED],
                NAMED,
            ),
            (vec!["--apparmor-profile"], NAMED),
        ] {
            assert!(
                !profile_flag(&words, profile).is_empty(),
                "{words:?} admitted for {profile}"
            );
        }

        let bootstrap = |words: &[&str], profile: &str| {
            bootstrap_profile(
                &words.iter().map(|word| json!(word)).collect::<Vec<_>>(),
                profile,
            )
        };
        assert!(bootstrap(&["--", "--socket", "s"], default).is_empty());
        assert!(
            bootstrap(
                &[
                    "--project-quotas",
                    "--apparmor-profile",
                    NAMED,
                    "--",
                    "--socket",
                    "s"
                ],
                NAMED
            )
            .is_empty()
        );
        assert!(!bootstrap(&["--", "--apparmor-profile", NAMED], NAMED).is_empty());
        assert!(
            bootstrap(
                &["--apparmor-profile=substrate-host-exec-v1-lane-a", "--"],
                NAMED
            )
            .is_empty()
        );
        assert!(!bootstrap(&["--apparmor-profile=", "--"], NAMED).is_empty());
        assert!(!bootstrap(&["--apparmor-profile=", "--"], default).is_empty());
        assert!(
            !bootstrap(
                &[
                    "--apparmor-profile",
                    NAMED,
                    "--",
                    "--apparmor-profile",
                    NAMED
                ],
                NAMED
            )
            .is_empty()
        );
    }

    /// `helm template` reaches no cluster: both renderers hand Helm an empty kubeconfig of their
    /// own, never the ambient one, whose contexts on a workstation may be production clusters.
    #[test]
    fn helm_template_is_given_an_empty_kubeconfig_of_its_own() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("temporary directory");
        let helm = directory.path().join("helm");
        // A child writes the stand-in, so no child another test forks inherits it open for
        // writing and fails its exec with `ETXTBSY`.
        let written = std::process::Command::new("/bin/sh")
            .args(["-c", "printf '%s' \"$1\" > \"$2\"", "stand-in"])
            .arg("#!/bin/sh\nif [ -n \"$KUBECONFIG\" ] && [ -f \"$KUBECONFIG\" ] && [ ! -s \"$KUBECONFIG\" ]; then echo empty; else echo \"ambient:$KUBECONFIG\"; fi\nenv | grep '^HELM_KUBE' || true\n")
            .arg(&helm)
            .stdin(std::process::Stdio::null())
            .status()
            .expect("the stand-in helm");
        assert!(
            written.success(),
            "writing the stand-in helm exited {written}"
        );
        std::fs::set_permissions(&helm, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        let chart = std::path::Path::new(ROOT);
        let rendered = super::render(&helm, chart, "lane", "lane", &json!({}))
            .expect("helm ran")
            .expect("helm succeeded");
        assert_eq!(rendered, "empty\n");
        let skipping = super::render_skipping_schema(&helm, chart, &json!({}))
            .expect("helm ran")
            .expect("helm succeeded");
        assert_eq!(skipping, "empty\n");
    }
}

#[cfg(test)]
#[path = "chart_adversary.rs"]
mod adversary;
