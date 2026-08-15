#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::Parser;
use substrate_daemon::{DaemonConfig, serve};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "substrated",
    version,
    about = "Daemonloom minimum substrate host"
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
}

impl From<Arguments> for DaemonConfig {
    fn from(arguments: Arguments) -> Self {
        Self {
            socket: arguments.socket,
            state: arguments.state,
            workspaces: arguments.workspaces,
            deployment: arguments.deployment,
            allow_uids: arguments.allow_uids,
            cgroup_root: arguments.cgroup_root,
            bubblewrap: arguments.bubblewrap,
            event_retention: arguments.event_retention,
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
