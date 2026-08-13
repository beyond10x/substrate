#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use axum::Extension;
use clap::Parser;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use substrate_daemon::{App, Identity, router};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::Store;
use tokio::net::UnixListener;
use tracing::{info, warn};
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let arguments = Arguments::parse();
    if arguments.allow_uids.is_empty() {
        bail!("at least one explicit --allow-uid mapping is required");
    }
    if arguments.deployment.is_empty()
        || !arguments
            .deployment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("deployment must be a non-empty stable identifier");
    }
    prepare_socket(&arguments.socket)?;
    if let Some(parent) = arguments.state.parent() {
        std::fs::create_dir_all(parent).context("create state directory")?;
    }
    let store = Arc::new(Store::open(&arguments.state).context("open durable state")?);
    let recovered = store
        .reconcile_after_restart()
        .context("reconcile durable operations")?;
    let mut host_config = HostConfig::minimum(&arguments.workspaces);
    host_config.cgroup_root = arguments.cgroup_root;
    host_config.bubblewrap = arguments.bubblewrap;
    let driver = HostDriver::open(host_config).context("open host driver")?;
    let app = App::new(store, driver, arguments.deployment);
    let listener = UnixListener::bind(&arguments.socket).context("bind unix socket")?;
    std::fs::set_permissions(&arguments.socket, std::fs::Permissions::from_mode(0o600))
        .context("restrict unix socket")?;
    let allowed = arguments.allow_uids.into_iter().collect::<BTreeSet<_>>();
    info!(socket = %arguments.socket.display(), recovered, "substrate ready");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accept unix peer")?;
                let credentials = stream.peer_cred().context("read unix peer credentials")?;
                let uid = credentials.uid();
                if !allowed.contains(&uid) {
                    warn!(uid, "refused unmapped unix peer");
                    continue;
                }
                let identity = Identity {
                    subject: format!("local:{uid}"),
                    actor: format!("unix-peer:{uid}"),
                    principal: credentials.pid().map(|pid| format!("pid:{pid}")),
                };
                let service = router(Arc::clone(&app)).layer(Extension(identity));
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(io, TowerToHyperService::new(service))
                        .await
                    {
                        warn!(%error, "unix HTTP connection failed");
                    }
                });
            }
            signal = tokio::signal::ctrl_c() => {
                signal.context("install shutdown signal")?;
                break;
            }
        }
    }
    drop(listener);
    std::fs::remove_file(&arguments.socket).context("remove unix socket")?;
    Ok(())
}

fn prepare_socket(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create socket directory")?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(path).context("remove stale unix socket")?;
        }
        Ok(_) => bail!("socket path exists and is not a unix socket"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect socket path"),
    }
    Ok(())
}
