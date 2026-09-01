#![forbid(unsafe_code)]

mod protocol;
mod surface;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::Arc;

use b10x_substrate_sdk::{Client, ManagedDaemon, SdkError};
use nix::fcntl::{Flock, FlockArg};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use ulid::Ulid;

pub use protocol::{MAX_CALLS, MAX_FRAME_BYTES, MAX_REQUEST_ID_BYTES, WAIT_LIMIT};

#[derive(Default)]
pub(crate) struct Registry {
    workspaces: BTreeSet<String>,
    execs: BTreeMap<String, String>,
    operations: BTreeSet<String>,
}

pub(crate) struct State {
    client: Client,
    registry: Mutex<Registry>,
}

impl State {
    fn new(client: Client) -> Self {
        Self {
            client,
            registry: Mutex::new(Registry::default()),
        }
    }
}

/// Run one disposable stdio MCP adapter until EOF or a termination signal.
///
/// # Errors
///
/// Returns a bounded diagnostic when startup, protocol service, resource cleanup, or child
/// shutdown cannot be proven.
pub async fn run() -> Result<(), String> {
    let deployment = format!("mcp_{}_{}", std::process::id(), Ulid::generate());
    let mut builder = ManagedDaemon::builder()
        .temporary()
        .deployment(deployment)
        .linked_current_exe();
    let delegated = delegated_cgroup_root()?;
    if let Some(root) = &delegated {
        builder = builder.cgroup_root(&root.path);
    }
    let mut daemon = builder
        .start()
        .await
        .map_err(|error| startup_error(&error))?;
    let client = daemon.client().clone();
    client
        .refresh_machine()
        .await
        .map_err(|error| startup_error(&error))?;
    let state = Arc::new(State::new(client));

    let served = protocol::serve(Arc::clone(&state)).await;
    let cleaned = cleanup(&state).await;
    let stopped = daemon
        .shutdown()
        .await
        .map_err(|error| shutdown_error(&error));
    drop(delegated);
    served.and(cleaned).and(stopped)
}

struct DelegatedRoot {
    path: PathBuf,
    _lock: Flock<File>,
}

fn delegated_cgroup_root() -> Result<Option<DelegatedRoot>, String> {
    let Some(value) = std::env::var_os("SUBSTRATE_MCP_CGROUP_ROOT") else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err("SUBSTRATE_MCP_CGROUP_ROOT must not be empty".to_owned());
    }
    let path = std::fs::canonicalize(PathBuf::from(value))
        .map_err(|_| "SUBSTRATE_MCP_CGROUP_ROOT must name an existing path".to_owned())?;
    let runtime =
        std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    if !runtime.is_absolute() {
        return Err("XDG_RUNTIME_DIR must be absolute".to_owned());
    }
    lock_delegated_root(path, &runtime).map(Some)
}

fn lock_delegated_root(path: PathBuf, runtime: &std::path::Path) -> Result<DelegatedRoot, String> {
    let digest = hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()));
    let lock_path = runtime.join(format!("b10x-substrate-mcp-cgroup-{digest}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW)
        .open(lock_path)
        .map_err(|_| "could not open delegated-root ownership lock".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "could not inspect delegated-root ownership lock".to_owned())?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err("delegated-root ownership lock is not owner-private".to_owned());
    }
    let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock)
        .map_err(|_| "delegated cgroup root is already owned by another adapter".to_owned())?;
    Ok(DelegatedRoot { path, _lock: lock })
}

async fn cleanup(state: &State) -> Result<(), String> {
    let (execs, workspaces) = {
        let registry = state.registry.lock().await;
        (
            registry.execs.keys().cloned().collect::<Vec<_>>(),
            registry.workspaces.iter().cloned().collect::<Vec<_>>(),
        )
    };
    let mut failures = 0_u32;
    for id in execs {
        match state.client.get_exec(&id).await {
            Ok(mut exec) => {
                if !exec.observation().state.terminal()
                    && exec
                        .signal_with_operation_id(
                            b10x_substrate_sdk::Signal::Kill,
                            std::time::Duration::from_secs(1),
                            Some(Ulid::generate().to_string()),
                        )
                        .await
                        .is_err()
                {
                    failures += 1;
                    continue;
                }
                if exec.wait_for(protocol::WAIT_LIMIT).await.is_err() {
                    failures += 1;
                    continue;
                }
                if exec
                    .retire_with_operation_id(Some(Ulid::generate().to_string()))
                    .await
                    .is_err()
                {
                    failures += 1;
                }
            }
            Err(SdkError::Refusal(refusal)) if refusal.code == "resource.not-found" => {}
            Err(_) => failures += 1,
        }
    }
    for id in workspaces {
        match state.client.get_workspace(&id).await {
            Ok(workspace) => {
                if workspace
                    .destroy_with_operation_id(Some(Ulid::generate().to_string()))
                    .await
                    .is_err()
                {
                    failures += 1;
                }
            }
            Err(SdkError::Refusal(refusal)) if refusal.code == "resource.not-found" => {}
            Err(_) => failures += 1,
        }
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(format!(
            "cleanup could not prove {failures} resource outcomes"
        ))
    }
}

fn startup_error(error: &SdkError) -> String {
    format!("startup refused: {}", safe_sdk_class(error))
}

fn shutdown_error(error: &SdkError) -> String {
    format!(
        "shutdown could not prove child absence: {}",
        safe_sdk_class(error)
    )
}

fn safe_sdk_class(error: &SdkError) -> &'static str {
    match error {
        SdkError::Transport(_) => "transport",
        SdkError::Refusal(_) => "daemon-refusal",
        SdkError::Protocol(_) => "protocol",
        SdkError::Builder { .. } => "builder",
        SdkError::UnknownOperation { .. } => "unknown-operation",
        SdkError::EventGap { .. } => "event-gap",
        SdkError::ContractMismatch { .. } => "contract-mismatch",
        SdkError::Startup(_) => "managed-startup",
        SdkError::Shutdown(_) => "managed-shutdown",
        _ => "sdk",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::lock_delegated_root;

    #[test]
    fn one_delegated_root_has_one_live_adapter_owner() {
        let runtime = tempfile::tempdir().expect("private runtime");
        let root = PathBuf::from("/sys/fs/cgroup/example-delegation");
        let owner =
            lock_delegated_root(root.clone(), runtime.path()).expect("first owner takes the lock");
        assert!(lock_delegated_root(root.clone(), runtime.path()).is_err());
        drop(owner);
        lock_delegated_root(root, runtime.path()).expect("the lock is released with its owner");
    }
}
