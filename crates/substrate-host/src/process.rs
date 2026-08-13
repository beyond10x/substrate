use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use parking_lot::Mutex;
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, Base64Content, Base64Encoding,
    BaselineEnvironment, CapabilitySnapshot, Exec, ExecExit, ExecKind, ExecOutputQuery,
    ExecSignalInput, ExecStartInput, ExecState, NetworkMode, OutputSlice, OutputStream,
    SandboxProfile, Signal,
};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::{Child, Command};
use tokio::sync::Notify;

use crate::{DriverError, HostConfig};

const TRUNCATION_MARKER: &[u8] = b"\n[substrate: output truncated]\n";

#[derive(Debug, Clone)]
pub struct ExecObservation {
    pub resource: Exec,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub output_complete: bool,
    pub cgroup: Option<String>,
    pub leader_pid: Option<u32>,
}

struct Execution {
    observation: Mutex<ExecObservation>,
    notify: Notify,
    cancellation_requested: AtomicBool,
}

impl Execution {
    fn new(observation: ExecObservation) -> Self {
        Self {
            observation: Mutex::new(observation),
            notify: Notify::new(),
            cancellation_requested: AtomicBool::new(false),
        }
    }
}

pub struct ProcessRuntime {
    config: HostConfig,
    capability: CapabilitySnapshot,
    executions: Arc<Mutex<HashMap<String, Arc<Execution>>>>,
    active: Arc<AtomicUsize>,
}

impl ProcessRuntime {
    pub fn new(config: HostConfig, capability: CapabilitySnapshot) -> Result<Self, DriverError> {
        let runtime = Self {
            config,
            capability,
            executions: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(AtomicUsize::new(0)),
        };
        runtime.reconcile_orphans()?;
        Ok(runtime)
    }

    fn reconcile_orphans(&self) -> Result<(), DriverError> {
        let Some(root) = self.config.cgroup_root.as_deref() else {
            return Ok(());
        };
        let entries = std::fs::read_dir(root).map_err(|error| {
            DriverError::failed(
                "exec.cgroup-read-failed",
                format!("cgroup delegation: {error}"),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                DriverError::failed("exec.cgroup-read-failed", format!("cgroup entry: {error}"))
            })?;
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if !name.starts_with("substrate-ex_") {
                continue;
            }
            let cgroup = Cgroup::existing(root, &name)?;
            cgroup.kill_all()?;
            let mut reconciled = false;
            for _ in 0..20 {
                if cgroup.prove_empty_and_remove().is_ok() {
                    reconciled = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            if !reconciled {
                return Err(DriverError::failed(
                    "exec.cgroup-not-empty",
                    "An orphaned exec cgroup could not be proven empty.",
                ));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Admission, spawn barrier, and durable observation stay adjacent.
    pub async fn start(
        &self,
        id: &str,
        workspace: &Path,
        input: &ExecStartInput,
    ) -> Result<ExecObservation, DriverError> {
        self.admit(id, workspace, input)?;
        let permit =
            ActivePermit::acquire(Arc::clone(&self.active), self.config.max_concurrent_execs)?;
        let cgroup = Cgroup::create(
            self.config
                .cgroup_root
                .as_deref()
                .ok_or_else(sandbox_unavailable)?,
            id,
            input,
        )?;
        let (sync_read, sync_write) = match pipe() {
            Ok(pipe) => pipe,
            Err(error) => {
                let _ = cgroup.prove_empty_and_remove();
                return Err(error);
            }
        };
        let sync_fd = sync_read.as_raw_fd();
        let mut command = self.command(workspace, input, sync_fd);
        let write_fd = sync_write.as_raw_fd();
        // SAFETY: pre_exec runs after fork; it invokes only async-signal-safe libc calls and does
        // not allocate. The captured descriptor is a plain integer owned by the parent until spawn.
        unsafe {
            command.as_std_mut().pre_exec(move || {
                if libc::close(write_fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if sync_fd > 3
                    && libc::syscall(
                        libc::SYS_close_range,
                        3_u32,
                        u32::try_from(sync_fd - 1).expect("descriptor is positive"),
                        0_u32,
                    ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::syscall(
                    libc::SYS_close_range,
                    u32::try_from(sync_fd + 1).expect("descriptor is positive"),
                    u32::MAX,
                    0_u32,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = cgroup.prove_empty_and_remove();
                return Err(DriverError::failed(
                    "exec.spawn-failed",
                    format!("bubblewrap spawn failed: {error}"),
                ));
            }
        };
        drop(sync_read);
        let leader_pid = child.id().ok_or_else(|| {
            DriverError::failed("exec.spawn-failed", "spawn returned no process identity")
        })?;
        if let Err(error) = cgroup.attach_tree(leader_pid) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = cgroup.prove_empty_and_remove();
            return Err(error);
        }
        if let Err(error) = release_barrier(&sync_write) {
            let _ = cgroup.kill_all();
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = cgroup.prove_empty_and_remove();
            return Err(error);
        }
        drop(sync_write);

        let applied = AppliedConfinement {
            capability_snapshot: self.capability.snapshot.clone(),
            cgroup: cgroup.name().to_owned(),
            filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
            network: AppliedNetwork::None,
            profile: SandboxProfile::Workspace,
        };
        let resource = Exec {
            id: id.to_owned(),
            kind: ExecKind::Exec,
            workspace: input.workspace.clone(),
            state: ExecState::Running,
            observed_at: Utc::now(),
            requested: input.sandbox.clone(),
            applied: Some(applied),
            exit: None,
        };
        let observation = ExecObservation {
            resource,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: false,
            cgroup: Some(cgroup.name().to_owned()),
            leader_pid: Some(leader_pid),
        };
        let execution = Arc::new(Execution::new(observation.clone()));
        let insertion = {
            let mut executions = self.executions.lock();
            if executions.len() >= self.config.max_tracked_execs {
                Err(DriverError::exhausted(
                    "exec.tracking-capacity",
                    "The bounded exec observation capacity is exhausted.",
                    "exec",
                ))
            } else if executions.contains_key(id) {
                Err(DriverError {
                    class: crate::DriverErrorClass::Conflict,
                    code: "exec.already-exists",
                    message: "Exec identity already exists.".to_owned(),
                    address: Some("exec".to_owned()),
                    retriable: false,
                })
            } else {
                executions.insert(id.to_owned(), Arc::clone(&execution));
                Ok(())
            }
        };
        if let Err(error) = insertion {
            let _ = cgroup.kill_all();
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = reconcile_cgroup(&cgroup).await;
            return Err(error);
        }
        let timeout = Duration::from_millis(input.limits.timeout_ms);
        let output_limit = usize::try_from(input.limits.output_bytes)
            .unwrap_or(usize::MAX)
            .min(usize::try_from(self.config.output_limit_bytes).unwrap_or(usize::MAX));
        tokio::spawn(run_child(
            child,
            cgroup,
            Arc::clone(&execution),
            timeout,
            output_limit,
            permit,
        ));
        if input.wait {
            wait_terminal(&execution, timeout.saturating_add(Duration::from_secs(5))).await?;
            Ok(execution.observation.lock().clone())
        } else {
            Ok(observation)
        }
    }

    pub fn observe(&self, id: &str) -> Result<ExecObservation, DriverError> {
        let execution = self
            .executions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(DriverError::not_found)?;
        let observation = execution.observation.lock().clone();
        if is_terminal(observation.resource.state) {
            self.executions.lock().remove(id);
        }
        Ok(observation)
    }

    pub fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError> {
        if query.limit_bytes > self.config.output_limit_bytes {
            return Err(DriverError::exhausted(
                "exec.output-limit",
                "Requested output range exceeds the probed limit.",
                "limit",
            ));
        }
        let observation = self.observe(id)?;
        let (source, truncated) = match query.stream {
            OutputStream::Stdout => (&observation.stdout, observation.stdout_truncated),
            OutputStream::Stderr => (&observation.stderr, observation.stderr_truncated),
        };
        let start = usize::try_from(query.offset)
            .unwrap_or(usize::MAX)
            .min(source.len());
        let limit = usize::try_from(query.limit_bytes).unwrap_or(usize::MAX);
        let end = start.saturating_add(limit).min(source.len());
        let next_offset = u64::try_from(end).expect("usize fits u64");
        Ok(OutputSlice {
            exec: id.to_owned(),
            stream: query.stream,
            offset: query.offset,
            returned_bytes: u64::try_from(end - start).expect("usize fits u64"),
            next_offset,
            eof: observation.output_complete && end == source.len(),
            truncated,
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(&source[start..end]),
            },
            observed_at: Utc::now(),
        })
    }

    pub async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        let execution = self
            .executions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(DriverError::not_found)?;
        if is_terminal(execution.observation.lock().resource.state) {
            return Ok(execution.observation.lock().clone());
        }
        let cgroup_name = execution
            .observation
            .lock()
            .cgroup
            .clone()
            .ok_or_else(|| DriverError::failed("exec.cgroup-missing", "Exec has no cgroup"))?;
        let leader_pid = execution.observation.lock().leader_pid;
        let cgroup_root = self
            .config
            .cgroup_root
            .as_deref()
            .ok_or_else(sandbox_unavailable)?;
        let cgroup = Cgroup::existing(cgroup_root, &cgroup_name)?;
        execution
            .cancellation_requested
            .store(true, Ordering::Release);
        match input.signal {
            Signal::Kill => cgroup.kill_all()?,
            signal => {
                cgroup.signal_all(signal, leader_pid)?;
                let grace = Duration::from_millis(input.grace_ms);
                if wait_terminal(&execution, grace).await.is_err() {
                    cgroup.kill_all()?;
                }
            }
        }
        wait_terminal(&execution, Duration::from_secs(5)).await?;
        let observation = execution.observation.lock().clone();
        Ok(observation)
    }

    fn admit(&self, id: &str, workspace: &Path, input: &ExecStartInput) -> Result<(), DriverError> {
        if !id.starts_with("ex_")
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(DriverError::refused(
                "exec.identity-invalid",
                "Exec identity is invalid.",
                "exec",
            ));
        }
        if input.sandbox.capability_snapshot != self.capability.snapshot {
            return Err(DriverError::refused(
                "exec.capability-stale",
                "The admitted capability snapshot is stale.",
                "capability_snapshot",
            ));
        }
        if !workspace.is_dir() {
            return Err(DriverError::not_found());
        }
        if !input.sandbox.required || input.sandbox.profile != SandboxProfile::Workspace {
            return Err(DriverError::refused(
                "exec.sandbox-invalid",
                "Phase 2 requires the workspace sandbox.",
                "sandbox",
            ));
        }
        if input.sandbox.network == NetworkMode::Aperture {
            return Err(DriverError::unserved(
                "exec.network-unserved",
                "Requested network aperture is not served by this host.",
                "exec.network-aperture",
            ));
        }
        if self.capability.facts.exec_namespaces.is_none()
            || self.capability.facts.exec_cgroup_limits.is_none()
            || self.capability.facts.exec_cgroup_kill.is_none()
        {
            return Err(sandbox_unavailable());
        }
        if input.argv.is_empty()
            || input.argv.len() > 256
            || input.argv.iter().any(|argument| {
                argument.is_empty() || argument.contains('\0') || argument.len() > 4096
            })
        {
            return Err(DriverError::refused(
                "exec.argv-invalid",
                "Exec argv is outside the closed bounds.",
                "argv",
            ));
        }
        if input.limits.timeout_ms == 0
            || input.limits.timeout_ms > 86_400_000
            || input.limits.output_bytes == 0
            || input.limits.output_bytes > self.config.output_limit_bytes
            || input.limits.processes == 0
            || input.limits.memory_bytes < 1_048_576
            || input.limits.cpu_millis == 0
        {
            return Err(DriverError::exhausted(
                "exec.limit-unserved",
                "Exec bounds exceed the probed host limits.",
                "limits",
            ));
        }
        validate_environment(input)?;
        self.recheck_backend()
    }

    fn recheck_backend(&self) -> Result<(), DriverError> {
        if !self.config.bubblewrap.is_file() {
            return Err(DriverError::refused(
                "exec.capability-stale",
                "Bubblewrap disappeared after capability admission.",
                "exec.namespaces",
            ));
        }
        let Some(root) = self.config.cgroup_root.as_deref() else {
            return Err(sandbox_unavailable());
        };
        if !root.join("cgroup.procs").is_file() {
            return Err(DriverError::refused(
                "exec.capability-stale",
                "Cgroup delegation disappeared after capability admission.",
                "exec.cgroup-limits",
            ));
        }
        Ok(())
    }

    fn command(&self, workspace: &Path, input: &ExecStartInput, sync_fd: RawFd) -> Command {
        let mut command = Command::new(&self.config.bubblewrap);
        command
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args([
                "--unshare-user",
                "--unshare-ipc",
                "--unshare-pid",
                "--unshare-net",
                "--unshare-uts",
                "--new-session",
                "--die-with-parent",
                "--clearenv",
                "--ro-bind",
                "/usr",
                "/usr",
                "--ro-bind-try",
                "/bin",
                "/bin",
                "--ro-bind-try",
                "/lib",
                "/lib",
                "--ro-bind-try",
                "/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--bind",
            ])
            .arg(workspace)
            .args(["/workspace", "--chdir", "/workspace", "--block-fd"])
            .arg(sync_fd.to_string());
        for name in &input.env.allow {
            command.args(["--setenv", name.as_str(), baseline_value(*name)]);
        }
        for (name, value) in &input.env.set {
            command.args(["--setenv", name, value]);
        }
        // Bubblewrap injects PWD after `--clearenv`; remove that implementation detail before
        // exec, then restore it only when the closed request explicitly supplied PWD.
        command.arg("--").args(["/usr/bin/env", "-u", "PWD"]);
        if let Some(value) = input.env.set.get("PWD") {
            command.arg(format!("PWD={value}"));
        }
        command.arg("--").args(&input.argv);
        command
    }
}

async fn run_child(
    mut child: Child,
    cgroup: Cgroup,
    execution: Arc<Execution>,
    timeout: Duration,
    output_limit: usize,
    _permit: ActivePermit,
) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move { drain_capped(stdout, output_limit).await });
    let stderr_task = tokio::spawn(async move { drain_capped(stderr, output_limit).await });
    let timed_out;
    let status = if let Ok(result) = tokio::time::timeout(timeout, child.wait()).await {
        timed_out = false;
        result
    } else {
        timed_out = true;
        let _ = cgroup.kill_all();
        child.wait().await
    };
    let (stdout, stdout_truncated) = stdout_task.await.unwrap_or_else(|_| (Vec::new(), true));
    let (stderr, stderr_truncated) = stderr_task.await.unwrap_or_else(|_| (Vec::new(), true));
    let cancellation = timed_out || execution.cancellation_requested.load(Ordering::Acquire);
    let cgroup_reconciled = reconcile_cgroup(&cgroup).await;
    let mut observation = execution.observation.lock();
    observation.stdout = stdout;
    observation.stderr = stderr;
    observation.stdout_truncated = stdout_truncated;
    observation.stderr_truncated = stderr_truncated;
    observation.output_complete = true;
    observation.resource.observed_at = Utc::now();
    match status {
        _ if !cgroup_reconciled => {
            observation.resource.state = ExecState::Unknown;
            observation.resource.exit = None;
        }
        Ok(status) => {
            let signal = status.signal().and_then(signal_from_number);
            observation.resource.exit = Some(ExecExit {
                code: status.code().and_then(|code| u8::try_from(code).ok()),
                signal,
            });
            observation.resource.state = if cancellation {
                ExecState::Cancelled
            } else {
                ExecState::Exited
            };
        }
        Err(_) => {
            observation.resource.state = ExecState::Unknown;
            observation.resource.exit = None;
        }
    }
    drop(observation);
    execution.notify.notify_waiters();
}

struct ActivePermit {
    active: Arc<AtomicUsize>,
}

impl ActivePermit {
    fn acquire(active: Arc<AtomicUsize>, maximum: usize) -> Result<Self, DriverError> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < maximum).then_some(current + 1)
            })
            .map_err(|_| {
                DriverError::exhausted(
                    "exec.concurrency-limit",
                    "The bounded exec concurrency limit is exhausted.",
                    "exec",
                )
            })?;
        Ok(Self { active })
    }
}

impl Drop for ActivePermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn reconcile_cgroup(cgroup: &Cgroup) -> bool {
    for attempt in 0..10 {
        if cgroup.prove_empty_and_remove().is_ok() {
            return true;
        }
        let _ = cgroup.kill_all();
        if attempt < 9 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    false
}

async fn drain_capped<R>(reader: Option<R>, limit: usize) -> (Vec<u8>, bool)
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return (Vec::new(), false);
    };
    let mut stored = Vec::with_capacity(limit.min(65_536));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => {
                truncated = true;
                break;
            }
        };
        let remaining = limit.saturating_sub(stored.len());
        let retained = remaining.min(count);
        stored.extend_from_slice(&buffer[..retained]);
        if retained < count {
            truncated = true;
        }
    }
    if truncated {
        if limit >= TRUNCATION_MARKER.len() {
            stored.truncate(limit - TRUNCATION_MARKER.len());
            stored.extend_from_slice(TRUNCATION_MARKER);
        } else {
            stored.truncate(limit);
        }
    }
    (stored, truncated)
}

async fn wait_terminal(execution: &Execution, limit: Duration) -> Result<(), DriverError> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let notified = execution.notify.notified();
        if is_terminal(execution.observation.lock().resource.state) {
            return Ok(());
        }
        tokio::time::timeout_at(deadline, notified)
            .await
            .map_err(|_| {
                DriverError::failed("exec.observe-timeout", "Exec did not become observable")
            })?;
    }
}

const fn is_terminal(state: ExecState) -> bool {
    matches!(
        state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Unknown
    )
}

fn validate_environment(input: &ExecStartInput) -> Result<(), DriverError> {
    if input.env.allow.len() > 5 || input.env.set.len() > 64 {
        return Err(environment_refused());
    }
    for (name, value) in &input.env.set {
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().enumerate().all(|(index, byte)| {
                if index == 0 {
                    byte.is_ascii_uppercase()
                } else {
                    byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                }
            })
            || value.len() > 4096
            || value.contains('\0')
            || is_secretish_name(name)
        {
            return Err(environment_refused());
        }
    }
    Ok(())
}

fn is_secretish_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "authorization",
        "bearer",
        "credential",
        "password",
        "proxy",
        "secret",
        "token",
    ]
    .iter()
    .any(|fragment| lower.contains(fragment))
}

fn environment_refused() -> DriverError {
    DriverError::refused(
        "exec.environment-refused",
        "Exec environment is outside the closed non-secret shape.",
        "env",
    )
}

const fn baseline_value(name: BaselineEnvironment) -> &'static str {
    match name {
        BaselineEnvironment::Lang | BaselineEnvironment::LcAll => "C.UTF-8",
        BaselineEnvironment::Path => "/usr/bin:/bin",
        BaselineEnvironment::Term => "dumb",
        BaselineEnvironment::Tz => "UTC",
    }
}

fn signal_from_number(number: i32) -> Option<Signal> {
    match number {
        libc::SIGINT => Some(Signal::Int),
        libc::SIGTERM => Some(Signal::Term),
        libc::SIGKILL => Some(Signal::Kill),
        _ => None,
    }
}

fn sandbox_unavailable() -> DriverError {
    DriverError::unserved(
        "exec.sandbox-unavailable",
        "Required host confinement is not available.",
        "exec.namespaces",
    )
}

struct Cgroup {
    path: PathBuf,
    name: String,
}

impl Cgroup {
    fn create(root: &Path, id: &str, input: &ExecStartInput) -> Result<Self, DriverError> {
        let name = format!("substrate-{id}");
        let path = root.join(&name);
        std::fs::create_dir(&path).map_err(|error| {
            DriverError::failed(
                "exec.cgroup-create-failed",
                format!("cgroup create: {error}"),
            )
        })?;
        let result = (|| {
            write_control(&path, "pids.max", &input.limits.processes.to_string())?;
            write_control(&path, "memory.max", &input.limits.memory_bytes.to_string())?;
            write_control(&path, "memory.swap.max", "0")?;
            let period = 100_000_u64;
            let quota = input
                .limits
                .cpu_millis
                .saturating_mul(period)
                .checked_div(input.limits.timeout_ms)
                .unwrap_or(period)
                .clamp(1_000, period);
            write_control(&path, "cpu.max", &format!("{quota} {period}"))?;
            if !path.join("cgroup.kill").is_file() {
                return Err(sandbox_unavailable());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir(&path);
        }
        result?;
        Ok(Self { path, name })
    }

    fn existing(root: &Path, name: &str) -> Result<Self, DriverError> {
        if !name.starts_with("substrate-ex_")
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(DriverError::refused(
                "exec.cgroup-invalid",
                "Stored cgroup identity is invalid.",
                "cgroup",
            ));
        }
        let path = root.join(name);
        if !path.join("cgroup.procs").is_file() {
            return Err(DriverError::not_found());
        }
        Ok(Self {
            path,
            name: name.to_owned(),
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn attach(&self, pid: u32) -> Result<(), DriverError> {
        write_control(&self.path, "cgroup.procs", &pid.to_string())
    }

    fn attach_tree(&self, leader: u32) -> Result<(), DriverError> {
        self.attach(leader)?;
        for process in process_tree(leader)? {
            self.attach(process)?;
        }
        Ok(())
    }

    fn kill_all(&self) -> Result<(), DriverError> {
        write_control(&self.path, "cgroup.kill", "1")
    }

    fn signal_all(&self, signal: Signal, excluded_pid: Option<u32>) -> Result<(), DriverError> {
        let processes =
            std::fs::read_to_string(self.path.join("cgroup.procs")).map_err(|error| {
                DriverError::failed(
                    "exec.cgroup-read-failed",
                    format!("cgroup processes: {error}"),
                )
            })?;
        for process in processes.lines() {
            let Ok(pid) = process.parse::<libc::pid_t>() else {
                continue;
            };
            if u32::try_from(pid).ok() == excluded_pid {
                continue;
            }
            // SAFETY: kill is called with a kernel-provided pid and a closed signal enum.
            let result = unsafe { libc::kill(pid, signal.number()) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err(DriverError::failed(
                    "exec.signal-failed",
                    format!("signal {} failed", signal.number()),
                ));
            }
        }
        Ok(())
    }

    fn prove_empty_and_remove(&self) -> Result<(), DriverError> {
        let processes =
            std::fs::read_to_string(self.path.join("cgroup.procs")).map_err(|error| {
                DriverError::failed(
                    "exec.cgroup-read-failed",
                    format!("cgroup processes: {error}"),
                )
            })?;
        if !processes.trim().is_empty() {
            return Err(DriverError::failed(
                "exec.cgroup-not-empty",
                "Exec cgroup still contains processes after observation.",
            ));
        }
        std::fs::remove_dir(&self.path).map_err(|error| {
            DriverError::failed(
                "exec.cgroup-cleanup-failed",
                format!("cgroup cleanup: {error}"),
            )
        })
    }
}

fn write_control(path: &Path, name: &str, value: &str) -> Result<(), DriverError> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path.join(name))
        .map_err(|error| {
            DriverError::failed("exec.cgroup-write-failed", format!("{name}: {error}"))
        })?;
    file.write_all(value.as_bytes()).map_err(|error| {
        DriverError::failed("exec.cgroup-write-failed", format!("{name}: {error}"))
    })
}

fn pipe() -> Result<(OwnedFd, OwnedFd), DriverError> {
    let mut descriptors = [-1; 2];
    // SAFETY: descriptors points to two writable integers; pipe2 initializes both on success.
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), 0) } != 0 {
        return Err(DriverError::failed(
            "exec.spawn-failed",
            format!("sync pipe: {}", std::io::Error::last_os_error()),
        ));
    }
    // SAFETY: successful pipe2 returns two new descriptors owned by this function.
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    })
}

fn release_barrier(descriptor: &OwnedFd) -> Result<(), DriverError> {
    let byte = [1_u8];
    // SAFETY: descriptor is the live write end of the private launch-barrier pipe.
    if unsafe { libc::write(descriptor.as_raw_fd(), byte.as_ptr().cast(), byte.len()) } != 1 {
        return Err(DriverError::failed(
            "exec.spawn-failed",
            format!("launch barrier: {}", std::io::Error::last_os_error()),
        ));
    }
    Ok(())
}

fn process_tree(leader: u32) -> Result<Vec<u32>, DriverError> {
    let mut discovered = Vec::new();
    let mut pending = vec![leader];
    while let Some(parent) = pending.pop() {
        let children =
            match std::fs::read_to_string(format!("/proc/{parent}/task/{parent}/children")) {
                Ok(children) => children,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(DriverError::failed(
                        "exec.process-tree-failed",
                        format!("process-tree observation failed: {error}"),
                    ));
                }
            };
        for child in children.split_whitespace() {
            let child = child.parse::<u32>().map_err(|_| {
                DriverError::failed(
                    "exec.process-tree-failed",
                    "Kernel process-tree observation was malformed.",
                )
            })?;
            if !discovered.contains(&child) {
                discovered.push(child);
                pending.push(child);
            }
        }
    }
    Ok(discovered)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use substrate_wire::{
        CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts, ExecEnvironment, ExecLimits,
        ExecStartInput, HostDriverKind, NamespaceFacts, NetworkMode, SandboxProfile,
    };

    use super::{ProcessRuntime, is_secretish_name};
    use crate::HostConfig;

    #[test]
    fn secret_shaped_environment_names_are_never_admitted() {
        for name in [
            "AUTHORIZATION",
            "MY_TOKEN",
            "DATABASE_PASSWORD",
            "HTTP_PROXY",
            "CREDENTIAL_FILE",
        ] {
            assert!(is_secretish_name(name), "{name}");
        }
        assert!(!is_secretish_name("VECTOR_VISIBLE"));
    }

    #[tokio::test]
    async fn stale_snapshot_refuses_before_backend_access() {
        let config = HostConfig::minimum("/does/not/exist");
        let capability = CapabilitySnapshot {
            snapshot: format!("sha256:{}", "7".repeat(64)),
            driver: HostDriverKind::Host,
            driver_version: "test".to_owned(),
            config_generation: 7,
            probed_at: chrono::Utc::now(),
            valid_until: None,
            facts: CapabilityFacts {
                exec_argv_only: Some(true),
                exec_namespaces: Some(NamespaceFacts {
                    user: true,
                    mount: true,
                    pid: true,
                    ipc: true,
                    uts: true,
                    network: true,
                }),
                exec_no_egress: Some(true),
                exec_cgroup_limits: Some(CgroupLimitFacts {
                    processes: true,
                    memory: true,
                    cpu: true,
                }),
                exec_cgroup_kill: Some(true),
                exec_output_limit_bytes: Some(65_536),
                ..CapabilityFacts::default()
            },
        };
        let runtime = ProcessRuntime::new(config, capability).expect("runtime");
        let input = ExecStartInput {
            workspace: "ws_test".to_owned(),
            argv: vec!["/usr/bin/true".to_owned()],
            env: ExecEnvironment {
                allow: vec![],
                set: BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "8".repeat(64)),
                network: NetworkMode::None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            limits: ExecLimits {
                timeout_ms: 5_000,
                output_bytes: 65_536,
                processes: 16,
                memory_bytes: 67_108_864,
                cpu_millis: 1_000,
            },
            wait: false,
        };
        let error = runtime
            .start("ex_test", std::path::Path::new("/does/not/exist"), &input)
            .await
            .expect_err("stale snapshot");
        assert_eq!(error.code, "exec.capability-stale");
    }
}
