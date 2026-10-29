//! The driver's settings: the closed context extension [`crate::EXTENSION`] of the operator's
//! kubeconfig (design 34 § 3). Configuration, never a request: nothing here is reachable from the
//! wire.

use std::net::SocketAddr;

use serde_json::{Map, Value};
use substrate_kubernetes_client::is_dns_subdomain;

use crate::EXTENSION;

/// The uid every container runs as when the operator names none.
pub const DEFAULT_RUN_AS_USER: u32 = 65_532;
/// The API server's request timeout when the operator states none (ADR 0047 N9).
pub const DEFAULT_API_REQUEST_TIMEOUT_SECONDS: u64 = 60;
/// The published workspace bound when the operator states none.
pub const DEFAULT_MAX_CURRENT_WORKSPACES: u64 = 64;
/// The published exec bound when the operator states none.
pub const DEFAULT_MAX_CURRENT_EXECS: u64 = 32;
/// The probe period when the operator states none.
pub const DEFAULT_PROBE_INTERVAL_SECONDS: u64 = 600;

const MIB: u64 = 1_048_576;
const TIB: u64 = 1_099_511_627_776;

/// What the extension states, over its defaults, with the namespace the current context names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceConfig {
    /// The one namespace the driver addresses.
    pub namespace: String,
    /// The storage class of every PVC.
    pub storage_class: String,
    /// The size of every PVC, 1 MiB to 1 TiB.
    pub workspace_size_bytes: u64,
    /// The executor image, by digest.
    pub executor_image: String,
    /// The image every exec runs in, by digest.
    pub exec_image: String,
    /// The operator's namespace-wide default-deny `NetworkPolicy` (ADR 0047 N5).
    pub default_deny_policy: String,
    /// The address outside the cluster the no-egress probe dials.
    pub egress_probe_address: SocketAddr,
    /// The kubelet's `podPidsLimit`, as the operator states it.
    pub pod_pids_limit: u32,
    /// The kubelet's `containerLogMaxSize`, as the operator states it (N10).
    pub container_log_max_bytes: u64,
    /// The numeric uid every container runs as; never 0.
    pub run_as_user: u32,
    /// The API server's request timeout, as the operator states it (N9).
    pub api_request_timeout_seconds: u64,
    /// The published `workspace.max-current`.
    pub max_current_workspaces: u64,
    /// The published `exec.max-current`.
    pub max_current_execs: u64,
    /// The probe period and `valid_until` window, 60 to 3600 seconds.
    pub probe_interval_seconds: u64,
}

/// The kubeconfig loader's `settings` step: reads [`EXTENSION`] from the current context. The
/// extension is required, may appear once, and is a closed object: an unknown member is refused.
///
/// # Errors
///
/// The first fault, naming the member; never a credential.
pub fn settings(context: &Value, namespace: String) -> Result<NamespaceConfig, String> {
    let mut named = context["extensions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|extension| extension["name"] == EXTENSION);
    let extension = named
        .next()
        .ok_or_else(|| format!("the current context carries no {EXTENSION} extension"))?;
    if named.next().is_some() {
        return Err(format!(
            "the current context names the {EXTENSION} extension twice"
        ));
    }
    let members = extension["extension"]
        .as_object()
        .ok_or_else(|| format!("the {EXTENSION} extension is not an object"))?;
    if let Some(unknown) = members
        .keys()
        .find(|member| !MEMBERS.contains(&member.as_str()))
    {
        return Err(format!(
            "the {EXTENSION} extension has the unknown member {unknown:?}"
        ));
    }
    let reader = Members(members);
    Ok(NamespaceConfig {
        namespace,
        storage_class: reader.dns_name("storage_class")?,
        workspace_size_bytes: reader.bounded("workspace_size_bytes", MIB, TIB, None)?,
        executor_image: reader.image("executor_image")?,
        exec_image: reader.image("exec_image")?,
        default_deny_policy: reader.dns_name("default_deny_policy")?,
        egress_probe_address: reader.address("egress_probe_address")?,
        pod_pids_limit: reader.uid_like("pod_pids_limit", None)?,
        container_log_max_bytes: reader.bounded("container_log_max_bytes", 1, u64::MAX, None)?,
        run_as_user: reader.uid_like("run_as_user", Some(DEFAULT_RUN_AS_USER))?,
        api_request_timeout_seconds: reader.bounded(
            "api_request_timeout_seconds",
            1,
            3_600,
            Some(DEFAULT_API_REQUEST_TIMEOUT_SECONDS),
        )?,
        max_current_workspaces: reader.bounded(
            "max_current_workspaces",
            1,
            substrate_wire::MAX_CURRENT_WORKSPACES,
            Some(DEFAULT_MAX_CURRENT_WORKSPACES),
        )?,
        max_current_execs: reader.bounded(
            "max_current_execs",
            1,
            substrate_wire::MAX_CURRENT_EXECS,
            Some(DEFAULT_MAX_CURRENT_EXECS),
        )?,
        probe_interval_seconds: reader.bounded(
            "probe_interval_seconds",
            60,
            3_600,
            Some(DEFAULT_PROBE_INTERVAL_SECONDS),
        )?,
    })
}

/// Every member the extension admits.
const MEMBERS: [&str; 13] = [
    "storage_class",
    "workspace_size_bytes",
    "executor_image",
    "exec_image",
    "default_deny_policy",
    "egress_probe_address",
    "pod_pids_limit",
    "container_log_max_bytes",
    "run_as_user",
    "api_request_timeout_seconds",
    "max_current_workspaces",
    "max_current_execs",
    "probe_interval_seconds",
];

struct Members<'a>(&'a Map<String, Value>);

impl Members<'_> {
    fn required(&self, member: &str) -> Result<&Value, String> {
        self.0
            .get(member)
            .ok_or_else(|| format!("the {EXTENSION} extension names no {member}"))
    }

    fn text(&self, member: &str) -> Result<&str, String> {
        self.required(member)?
            .as_str()
            .ok_or_else(|| format!("{member} is not a string"))
    }

    fn dns_name(&self, member: &str) -> Result<String, String> {
        let name = self.text(member)?;
        if is_dns_subdomain(name) {
            Ok(name.to_owned())
        } else {
            Err(format!("{member} is not a DNS-1123 subdomain"))
        }
    }

    /// `…@sha256:<64 lowercase hex>`, with no tag in the repository's last path segment.
    fn image(&self, member: &str) -> Result<String, String> {
        let image = self.text(member)?;
        let untagged = image
            .split_once("@sha256:")
            .map(|(name, _)| name.rsplit('/').next().unwrap_or(name))
            .is_some_and(|repository| !repository.contains(':'));
        if substrate_host::confinement::image_is_pinned(image) && untagged {
            Ok(image.to_owned())
        } else {
            Err(format!(
                "{member} must name an image by digest, `name@sha256:<64 lowercase hex>`, \
                 without a tag"
            ))
        }
    }

    /// An `ip:port` that a Pod could dial outside the cluster: not loopback, not unspecified, not
    /// port 0.
    fn address(&self, member: &str) -> Result<SocketAddr, String> {
        let address: SocketAddr = self
            .text(member)?
            .parse()
            .map_err(|_| format!("{member} is not an ip:port"))?;
        if address.ip().is_loopback() || address.ip().is_unspecified() || address.port() == 0 {
            return Err(format!(
                "{member} must be an address outside the cluster, not loopback, unspecified or \
                 port 0"
            ));
        }
        Ok(address)
    }

    /// A positive 32-bit integer.
    fn uid_like(&self, member: &str, default: Option<u32>) -> Result<u32, String> {
        match (self.0.get(member), default) {
            (None, Some(default)) => Ok(default),
            _ => self
                .required(member)?
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("{member} must be a positive 32-bit integer")),
        }
    }

    fn bounded(
        &self,
        member: &str,
        low: u64,
        high: u64,
        default: Option<u64>,
    ) -> Result<u64, String> {
        match (self.0.get(member), default) {
            (None, Some(default)) => Ok(default),
            _ => self
                .required(member)?
                .as_u64()
                .filter(|value| (low..=high).contains(value))
                .ok_or_else(|| format!("{member} must be an integer from {low} to {high}")),
        }
    }
}
