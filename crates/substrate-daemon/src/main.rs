#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use substrate_daemon::{DaemonConfig, TcpDaemonConfig, serve};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "substrate-daemon",
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
