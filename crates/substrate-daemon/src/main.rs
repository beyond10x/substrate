#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use clap::Parser;
use substrate_daemon::{
    DaemonConfig, DelegatedContextKey, EgressAperture, SecretSlot, TcpDaemonConfig, serve,
};
use tracing_subscriber::EnvFilter;

/// Parses one `--secret-slot name=path`.
///
/// Splits at the **first** `=`, so a path may contain one. The name shape is the wire's own rule,
/// not a second copy of it.
fn parse_secret_slot(value: &str) -> Result<SecretSlot, String> {
    let (name, path) = value
        .split_once('=')
        .ok_or_else(|| "a secret slot is declared as name=path".to_owned())?;
    if !substrate_wire::valid_secret_slot_name(name) {
        return Err("a secret slot name must match [a-z][a-z0-9_]{0,63}".to_owned());
    }
    if path.is_empty() {
        return Err(format!("secret slot {name} declares no file"));
    }
    Ok(SecretSlot {
        name: name.to_owned(),
        path: PathBuf::from(path),
    })
}

/// Parses one `--egress-aperture name=host:port/tcp`.
///
/// The protocol suffix is required and is `tcp`, so a later slice that serves another one does not
/// silently reinterpret a declaration written today (design 10 § 9 decision 3).
fn parse_egress_aperture(value: &str) -> Result<EgressAperture, String> {
    let (name, destination) = value
        .split_once('=')
        .ok_or_else(|| "an egress aperture is declared as name=host:port/tcp".to_owned())?;
    if !substrate_wire::valid_aperture_name(name) {
        return Err("an egress aperture name must match [a-z][a-z0-9_]{0,63}".to_owned());
    }
    let destination = destination
        .strip_suffix("/tcp")
        .ok_or_else(|| format!("egress aperture {name} must declare a /tcp destination"))?;
    let (host, port) = destination
        .rsplit_once(':')
        .ok_or_else(|| format!("egress aperture {name} must declare host:port"))?;
    if host.is_empty() {
        return Err(format!("egress aperture {name} declares no host"));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("egress aperture {name} declares no usable port"))?;
    if port == 0 {
        return Err(format!("egress aperture {name} declares no usable port"));
    }
    Ok(EgressAperture {
        name: name.to_owned(),
        host: host.to_owned(),
        port,
    })
}

/// Parses one `--delegated-context-key kid=issuer=base64url-ed25519-public-key`.
///
/// Public material only. Substrate mints no delegated context, so there is no shape of this flag
/// that takes a seed or a signing key, and which service signs is exactly this declaration
/// (ADR 0011). The key is base64url without padding, so the value carries no `=` of its own and the
/// two splits are unambiguous.
fn parse_delegated_context_key(value: &str) -> Result<DelegatedContextKey, String> {
    let (kid, rest) = value.split_once('=').ok_or_else(|| {
        "a delegated-context key is declared as kid=issuer=base64url-public-key".to_owned()
    })?;
    let (issuer, encoded) = rest.split_once('=').ok_or_else(|| {
        "a delegated-context key is declared as kid=issuer=base64url-public-key".to_owned()
    })?;
    if kid.is_empty() || kid.len() > 128 {
        return Err("a delegated-context key id must be 1..=128 bytes".to_owned());
    }
    if issuer.is_empty() || issuer.len() > 512 {
        return Err(format!("delegated-context key {kid} declares no issuer"));
    }
    let raw = BASE64URL
        .decode(encoded)
        .map_err(|_| format!("delegated-context key {kid} is not unpadded base64url"))?;
    let public_key: [u8; 32] = raw
        .try_into()
        .map_err(|_| format!("delegated-context key {kid} is not a 32-byte Ed25519 key"))?;
    Ok(DelegatedContextKey {
        kid: kid.to_owned(),
        issuer: issuer.to_owned(),
        public_key,
    })
}

#[derive(Debug, Parser)]
#[command(
    name = "substrate-daemon",
    version,
    about = "b10x minimum substrate host"
)]
struct Arguments {
    #[arg(long, env = "SUBSTRATE_SOCKET")]
    socket: PathBuf,

    #[arg(long, env = "SUBSTRATE_STATE")]
    state: PathBuf,

    #[arg(long, env = "SUBSTRATE_WORKSPACES")]
    workspaces: PathBuf,

    #[arg(long, env = "SUBSTRATE_DEPLOYMENT")]
    deployment: String,

    #[arg(long = "allow-uid", env = "SUBSTRATE_ALLOW_UID", value_delimiter = ',')]
    allow_uids: Vec<u32>,

    #[arg(long, env = "SUBSTRATE_CGROUP_ROOT")]
    cgroup_root: Option<PathBuf>,

    #[arg(long, default_value = "/usr/bin/bwrap")]
    bubblewrap: PathBuf,

    #[arg(long, default_value_t = 10_000)]
    event_retention: u64,

    /// Declare a secret slot as `name=path` (repeatable). ADR 0012.
    ///
    /// The path never leaves this process: it is not a capability fact, not an event field and not
    /// an error message. Rotating the file behind a declared name needs no restart and invalidates
    /// no admitted operation.
    #[arg(
        long = "secret-slot",
        env = "SUBSTRATE_SECRET_SLOT",
        value_name = "NAME=PATH",
        value_delimiter = ',',
        value_parser = parse_secret_slot
    )]
    secret_slots: Vec<SecretSlot>,

    /// Declare an egress aperture as `name=host:port/tcp` (repeatable). ADR 0013.
    ///
    /// This is where reach is decided. A request may name one of these and may never carry a
    /// destination; the host is resolved once, here, and pinned for this process's lifetime.
    #[arg(
        long = "egress-aperture",
        env = "SUBSTRATE_EGRESS_APERTURE",
        value_name = "NAME=HOST:PORT/tcp",
        value_delimiter = ',',
        value_parser = parse_egress_aperture
    )]
    egress_apertures: Vec<EgressAperture>,

    /// A certificate bundle a run with an aperture gets a private read-only snapshot of.
    ///
    /// Without it a sandbox has no trust anchor — it has no `/etc` at all — so TLS crosses the
    /// aperture intact and fails verification inside. Absent and unverifiable, never present and
    /// unverified.
    #[arg(long, env = "SUBSTRATE_CA_BUNDLE", value_name = "PATH")]
    ca_bundle: Option<PathBuf>,

    /// Trust a delegated-context signer as `kid=issuer=base64url-public-key` (repeatable). ADR 0011.
    ///
    /// This is the whole of "who signs". Substrate verifies the binding a document declares and
    /// records the grant it names; it never evaluates that grant and never calls the issuer.
    #[arg(
        long = "delegated-context-key",
        env = "SUBSTRATE_DELEGATED_CONTEXT_KEY",
        value_name = "KID=ISSUER=BASE64URL",
        value_delimiter = ',',
        value_parser = parse_delegated_context_key
    )]
    delegated_context_keys: Vec<DelegatedContextKey>,

    /// Refuse an effectful operation that presents no delegated context.
    ///
    /// Requires a trusted key: requiring what cannot be verified refuses every mutation, which
    /// startup rejects rather than serving.
    #[arg(
        long = "require-delegated-context",
        env = "SUBSTRATE_REQUIRE_DELEGATED_CONTEXT",
        requires = "delegated_context_keys"
    )]
    require_delegated_context: bool,

    #[arg(
        long,
        env = "SUBSTRATE_TCP_LISTEN",
        requires_all = ["tcp_bearer_file", "tcp_subject", "tcp_actor", "tcp_private_overlay"]
    )]
    tcp_listen: Option<SocketAddr>,

    #[arg(long, env = "SUBSTRATE_TCP_PATH_PREFIX", requires = "tcp_listen")]
    tcp_path_prefix: Option<String>,

    #[arg(long, env = "SUBSTRATE_TCP_BEARER_FILE", requires = "tcp_listen")]
    tcp_bearer_file: Option<PathBuf>,

    #[arg(long, env = "SUBSTRATE_TCP_SUBJECT", requires = "tcp_listen")]
    tcp_subject: Option<String>,

    #[arg(long, env = "SUBSTRATE_TCP_ACTOR", requires = "tcp_listen")]
    tcp_actor: Option<String>,

    #[arg(long, env = "SUBSTRATE_TCP_PRIVATE_OVERLAY", requires = "tcp_listen")]
    tcp_private_overlay: bool,

    #[arg(long, env = "SUBSTRATE_TCP_DEVELOPMENT_ONLY", requires = "tcp_listen")]
    tcp_development_only: bool,
}

impl From<Arguments> for DaemonConfig {
    fn from(arguments: Arguments) -> Self {
        let tcp = arguments.tcp_listen.map(|listen| TcpDaemonConfig {
            listen,
            path_prefix: arguments.tcp_path_prefix.unwrap_or_else(|| "/".to_owned()),
            bearer_file: arguments
                .tcp_bearer_file
                .expect("clap requires a bearer file with TCP"),
            subject: arguments
                .tcp_subject
                .expect("clap requires a subject with TCP"),
            actor: arguments
                .tcp_actor
                .expect("clap requires an actor with TCP"),
            private_overlay: arguments.tcp_private_overlay,
            development_only: arguments.tcp_development_only,
        });
        Self {
            socket: arguments.socket,
            state: arguments.state,
            workspaces: arguments.workspaces,
            deployment: arguments.deployment,
            allow_uids: arguments.allow_uids,
            cgroup_root: arguments.cgroup_root,
            bubblewrap: arguments.bubblewrap,
            event_retention: arguments.event_retention,
            secret_slots: arguments.secret_slots,
            egress_apertures: arguments.egress_apertures,
            ca_bundle: arguments.ca_bundle,
            delegated_context_keys: arguments.delegated_context_keys,
            require_delegated_context: arguments.require_delegated_context,
            tcp,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    serve(Arguments::parse().into()).await
}
