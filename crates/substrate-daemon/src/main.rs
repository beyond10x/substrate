#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use substrate_daemon::{DaemonConfig, SecretSlot, TcpDaemonConfig, serve};
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
