use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "linked-daemon")]
use base64::Engine as _;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::io::AsyncReadExt as _;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[cfg(feature = "linked-daemon")]
use crate::model::LinkedChildConfig;
use crate::{Client, SdkError};

const LINKED_CHILD_ENV: &str = "B10X_SUBSTRATE_LINKED_CHILD";
const DIAGNOSTIC_LIMIT: usize = 64 * 1024;

enum DaemonSource {
    External(PathBuf),
    #[cfg(feature = "linked-daemon")]
    LinkedCurrentExe,
}

#[must_use]
pub struct ManagedDaemonBuilder {
    data_dir: Option<PathBuf>,
    deployment: Option<String>,
    source: Option<DaemonSource>,
    temporary: bool,
    cgroup_root: Option<PathBuf>,
    bubblewrap: PathBuf,
    event_retention: u64,
    startup_timeout: Duration,
    shutdown_grace: Duration,
}

impl Default for ManagedDaemonBuilder {
    fn default() -> Self {
        Self {
            data_dir: None,
            deployment: None,
            source: None,
            temporary: false,
            cgroup_root: None,
            bubblewrap: PathBuf::from("/usr/bin/bwrap"),
            event_retention: 10_000,
            startup_timeout: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(10),
        }
    }
}

impl ManagedDaemonBuilder {
    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(path.into());
        self.temporary = false;
        self
    }

    pub fn temporary(mut self) -> Self {
        self.data_dir = None;
        self.temporary = true;
        self
    }

    pub fn deployment(mut self, deployment: impl Into<String>) -> Self {
        self.deployment = Some(deployment.into());
        self
    }

    pub fn external_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.source = Some(DaemonSource::External(binary.into()));
        self
    }

    #[cfg(feature = "linked-daemon")]
    pub fn linked_current_exe(mut self) -> Self {
        self.source = Some(DaemonSource::LinkedCurrentExe);
        self
    }

    pub fn cgroup_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.cgroup_root = Some(path.into());
        self
    }

    pub fn bubblewrap(mut self, path: impl Into<PathBuf>) -> Self {
        self.bubblewrap = path.into();
        self
    }

    pub fn event_retention(mut self, events: u64) -> Self {
        self.event_retention = events;
        self
    }

    pub fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    #[allow(
        clippy::too_many_lines,
        reason = "startup is one ordered ownership transfer with cleanup at every refusal"
    )]
    pub async fn start(self) -> Result<ManagedDaemon, SdkError> {
        let deployment = self.deployment.ok_or(SdkError::Builder {
            field: "deployment",
        })?;
        if deployment.is_empty()
            || !deployment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(SdkError::Protocol(
                "deployment must be a stable ASCII identifier".to_owned(),
            ));
        }
        let source = self.source.ok_or(SdkError::Builder {
            field: "daemon_source",
        })?;
        if self.event_retention == 0 {
            return Err(SdkError::Protocol(
                "event retention must be nonzero".to_owned(),
            ));
        }
        let temporary = if self.temporary {
            Some(
                tempfile::Builder::new()
                    .prefix("b10x-substrate-")
                    .tempdir()
                    .map_err(|error| SdkError::Startup(error.to_string()))?,
            )
        } else {
            None
        };
        let data_dir = match (&temporary, self.data_dir) {
            (Some(directory), _) => directory.path().to_owned(),
            (None, Some(path)) => path,
            (None, None) => return Err(SdkError::Builder { field: "data_dir" }),
        };
        std::fs::create_dir_all(&data_dir).map_err(|error| SdkError::Startup(error.to_string()))?;
        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| SdkError::Startup(error.to_string()))?;
        let socket = data_dir.join("substrate.sock");
        let state = data_dir.join("state.db");
        let workspaces = data_dir.join("workspaces");
        let uid = nix::unistd::geteuid().as_raw();
        #[cfg(feature = "linked-daemon")]
        let linked_config = LinkedChildConfig {
            socket: path_string(&socket)?,
            state: path_string(&state)?,
            workspaces: path_string(&workspaces)?,
            deployment: deployment.clone(),
            uid,
            cgroup_root: self
                .cgroup_root
                .as_ref()
                .map(|path| path_string(path))
                .transpose()?,
            bubblewrap: path_string(&self.bubblewrap)?,
            event_retention: self.event_retention,
        };
        let mut command = match source {
            DaemonSource::External(binary) => {
                let mut command = Command::new(binary);
                command
                    .arg("--socket")
                    .arg(&socket)
                    .arg("--state")
                    .arg(&state)
                    .arg("--workspaces")
                    .arg(&workspaces)
                    .arg("--deployment")
                    .arg(&deployment)
                    .arg("--allow-uid")
                    .arg(uid.to_string())
                    .arg("--bubblewrap")
                    .arg(&self.bubblewrap)
                    .arg("--event-retention")
                    .arg(self.event_retention.to_string())
                    .arg("--exit-on-stdin-close");
                if let Some(root) = &self.cgroup_root {
                    command.arg("--cgroup-root").arg(root);
                }
                command
            }
            #[cfg(feature = "linked-daemon")]
            DaemonSource::LinkedCurrentExe => {
                let current = std::env::current_exe()
                    .map_err(|error| SdkError::Startup(error.to_string()))?;
                let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&linked_config)
                        .map_err(|error| SdkError::Protocol(error.to_string()))?,
                );
                let mut command = Command::new(current);
                command.env(LINKED_CHILD_ENV, encoded);
                command
            }
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| SdkError::Startup(error.to_string()))?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut pipe) = child.stderr.take() {
            let diagnostics = Arc::clone(&stderr);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 4096];
                while let Ok(read) = pipe.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    let mut retained = diagnostics.lock().await;
                    let remaining = DIAGNOSTIC_LIMIT.saturating_sub(retained.len());
                    retained.extend_from_slice(&buffer[..read.min(remaining)]);
                }
            });
        }
        let deadline = tokio::time::Instant::now() + self.startup_timeout;
        let client = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| SdkError::Startup(error.to_string()))?
            {
                let diagnostics = diagnostics(&stderr).await;
                return Err(SdkError::Startup(format!(
                    "daemon exited with {status}: {diagnostics}"
                )));
            }
            match Client::builder().unix_socket(&socket).connect().await {
                Ok(client) => break client,
                Err(error @ SdkError::ContractMismatch { .. }) => {
                    let _ = terminate_child(&mut child, self.shutdown_grace).await;
                    return Err(error);
                }
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => {
                    let diagnostics = diagnostics(&stderr).await;
                    let _ = terminate_child(&mut child, self.shutdown_grace).await;
                    return Err(SdkError::Startup(format!(
                        "readiness deadline elapsed ({error}): {diagnostics}"
                    )));
                }
            }
        };
        Ok(ManagedDaemon {
            client,
            child: Some(child),
            shutdown_grace: self.shutdown_grace,
            temporary,
        })
    }
}

pub struct ManagedDaemon {
    client: Client,
    child: Option<Child>,
    shutdown_grace: Duration,
    temporary: Option<tempfile::TempDir>,
}

impl ManagedDaemon {
    pub fn builder() -> ManagedDaemonBuilder {
        ManagedDaemonBuilder::default()
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    pub async fn shutdown(mut self) -> Result<(), SdkError> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        terminate_child(&mut child, self.shutdown_grace).await?;
        self.temporary.take();
        Ok(())
    }
}

impl Drop for ManagedDaemon {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let grace = self.shutdown_grace;
        let temporary = self.temporary.take();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _temporary = temporary;
                let _ = terminate_child(&mut child, grace).await;
            });
        } else {
            std::thread::spawn(move || {
                let _temporary = temporary;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                if let Ok(runtime) = runtime {
                    let _ = runtime.block_on(terminate_child(&mut child, grace));
                } else {
                    let _ = child.start_kill();
                }
            });
        }
    }
}

async fn terminate_child(child: &mut Child, grace: Duration) -> Result<(), SdkError> {
    child.stdin.take();
    if let Some(id) = child.id() {
        let raw = i32::try_from(id)
            .map_err(|_| SdkError::Shutdown("child pid is outside i32".to_owned()))?;
        let _ = kill(Pid::from_raw(raw), Signal::SIGTERM);
    }
    if let Ok(result) = tokio::time::timeout(grace, child.wait()).await {
        return result
            .map(|_| ())
            .map_err(|error| SdkError::Shutdown(error.to_string()));
    }
    child
        .kill()
        .await
        .map_err(|error| SdkError::Shutdown(error.to_string()))?;
    child
        .wait()
        .await
        .map(|_| ())
        .map_err(|error| SdkError::Shutdown(error.to_string()))
}

async fn diagnostics(stderr: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&stderr.lock().await)
        .trim()
        .to_owned()
}

#[cfg(feature = "linked-daemon")]
fn path_string(path: &std::path::Path) -> Result<String, SdkError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| SdkError::Protocol("managed paths must be UTF-8".to_owned()))
}

/// Run the linked daemon child when this process was re-executed by [`ManagedDaemonBuilder`].
///
/// Applications enabling `linked-daemon` call this before parsing their own command line. `false`
/// means this is the ordinary parent process and application startup should continue.
pub async fn run_daemon_child_if_requested() -> Result<bool, SdkError> {
    let Some(encoded) = std::env::var_os(LINKED_CHILD_ENV) else {
        return Ok(false);
    };
    #[cfg(not(feature = "linked-daemon"))]
    {
        let _ = encoded;
        Err(SdkError::Startup(
            "linked child requested without the linked-daemon feature".to_owned(),
        ))
    }
    #[cfg(feature = "linked-daemon")]
    {
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded.as_encoded_bytes())
            .map_err(|error| SdkError::Startup(error.to_string()))?;
        let child: LinkedChildConfig =
            serde_json::from_slice(&raw).map_err(|error| SdkError::Startup(error.to_string()))?;
        let mut config = substrate_daemon::DaemonConfig::minimum(
            child.socket,
            child.state,
            child.workspaces,
            child.deployment,
            vec![child.uid],
        );
        config.cgroup_root = child.cgroup_root.map(PathBuf::from);
        config.bubblewrap = PathBuf::from(child.bubblewrap);
        config.event_retention = child.event_retention;
        let serve = substrate_daemon::serve(config);
        tokio::pin!(serve);
        let mut stdin = tokio::io::stdin();
        let mut byte = [0_u8; 1];
        tokio::select! {
            result = &mut serve => {
                result.map_err(|error| SdkError::Startup(error.to_string()))?;
            }
            result = stdin.read(&mut byte) => {
                result.map_err(|error| SdkError::Startup(error.to_string()))?;
                let pid = i32::try_from(std::process::id())
                    .map_err(|_| SdkError::Shutdown("process id is outside i32".to_owned()))?;
                kill(Pid::from_raw(pid), Signal::SIGTERM)
                    .map_err(|error| SdkError::Shutdown(error.to_string()))?;
                serve.await.map_err(|error| SdkError::Startup(error.to_string()))?;
            }
        }
        Ok(true)
    }
}
