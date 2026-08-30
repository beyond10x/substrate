use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use parking_lot::Mutex;
use substrate_wire::{
    AppliedConfinement, AppliedExecutionCapsule, AppliedFilesystem, AppliedNetwork, Base64Content,
    Base64Encoding, BaselineEnvironment, CapabilitySnapshot, EXECUTION_CAPSULE_MOUNT, Exec,
    ExecExit, ExecKind, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState,
    ExecutionCapsuleInput, OutputSlice, OutputStream, PipeSessionStartInput, PtyWindow,
    SandboxProfile, SessionMode, Signal,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Notify, mpsc};

use crate::{DispatchOutcome, DriverError, HostConfig};

const TRUNCATION_MARKER: &[u8] = b"\n[substrate: output truncated]\n";
const PIPE_FRAME_BYTES: usize = 64 * 1024;
const PIPE_QUEUED_FRAMES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeFrame {
    pub stream: PipeStream,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    delivered_signal: Mutex<Option<Signal>>,
    pipe_backpressure: AtomicBool,
    pipe: Option<PipeState>,
}

struct PipeState {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    output: tokio::sync::Mutex<mpsc::Receiver<PipeFrame>>,
    input_bytes: AtomicU64,
    input_limit: u64,
    frame_limit: usize,
    /// The master end, when this session carries a terminal rather than three pipes.
    ///
    /// Input goes here instead of to `stdin`, because a terminal has one descriptor and the line
    /// discipline on it is what turns a client's bytes into what the child reads.
    terminal: Option<Arc<crate::pty::PtyMaster>>,
}

#[derive(Debug, Clone, Copy)]
struct PipeSettings {
    input_limit: u64,
    frame_limit: usize,
    queued_frames: usize,
    /// The initial window, present exactly when this start asked for `mode: "pty"`.
    window: Option<PtyWindow>,
}

/// What the confined child's three standard descriptors are.
enum ChildChannel {
    /// No live channel: stdin is `/dev/null` and output is captured through pipes.
    Quiet,
    /// Three separately attributed pipes (ADR 0007).
    Pipes,
    /// One terminal, inherited as descriptors 0, 1 and 2. Stdout and stderr *are* the same file
    /// here, which is the mechanical reason ADR 0007 forbids running a machine protocol on one.
    Terminal(Box<[OwnedFd; 3]>),
}

struct PreparedCapsule {
    directory: tempfile::TempDir,
    applied: AppliedExecutionCapsule,
}

impl Execution {
    fn new(observation: ExecObservation, pipe: Option<PipeState>) -> Self {
        Self {
            observation: Mutex::new(observation),
            notify: Notify::new(),
            cancellation_requested: AtomicBool::new(false),
            delivered_signal: Mutex::new(None),
            pipe_backpressure: AtomicBool::new(false),
            pipe,
        }
    }
}

pub struct ProcessRuntime {
    config: HostConfig,
    capability: CapabilitySnapshot,
    backend_binding: Option<crate::probe::BackendBinding>,
    executions: Arc<Mutex<HashMap<String, Arc<Execution>>>>,
    reservations: Arc<Mutex<HashSet<String>>>,
    active: Arc<AtomicUsize>,
}

impl ProcessRuntime {
    pub fn new(config: HostConfig, capability: CapabilitySnapshot) -> Result<Self, DriverError> {
        let backend_binding = crate::probe::backend_binding(&config);
        let runtime = Self {
            config,
            capability,
            backend_binding,
            executions: Arc::new(Mutex::new(HashMap::new())),
            reservations: Arc::new(Mutex::new(HashSet::new())),
            active: Arc::new(AtomicUsize::new(0)),
        };
        let process_trees_reconciled = runtime.reconcile_orphans()?;
        runtime.reconcile_capsules(process_trees_reconciled)?;
        runtime.reconcile_apertures(process_trees_reconciled)?;
        Ok(runtime)
    }

    fn reconcile_orphans(&self) -> Result<bool, DriverError> {
        let Some(root) = self.config.cgroup_root.as_deref() else {
            return Ok(false);
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
        Ok(true)
    }

    fn reconcile_capsules(&self, process_trees_reconciled: bool) -> Result<(), DriverError> {
        let entries = match std::fs::read_dir(&self.config.capsule_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(DriverError::failed(
                    "capsule.reconcile-failed",
                    format!("capsule root: {error}"),
                ));
            }
        };
        let mut reconciled = 0_usize;
        for entry in entries {
            if !process_trees_reconciled {
                return Err(DriverError::failed(
                    "capsule.reconcile-unproven",
                    "Stale capsule cleanup requires successful cgroup-root reconciliation.",
                ));
            }
            let entry = entry.map_err(|error| {
                DriverError::failed(
                    "capsule.reconcile-failed",
                    format!("capsule entry: {error}"),
                )
            })?;
            reconciled = reconciled.checked_add(1).ok_or_else(|| {
                DriverError::failed(
                    "capsule.reconcile-failed",
                    "Capsule reconciliation count overflowed.",
                )
            })?;
            if reconciled > self.config.max_tracked_execs {
                return Err(DriverError::failed(
                    "capsule.reconcile-limit",
                    "Stale capsule count exceeds the configured tracked-exec bound.",
                ));
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(DriverError::failed(
                    "capsule.reconcile-invalid",
                    "A stale capsule has a non-UTF-8 name.",
                ));
            };
            if !name.starts_with("capsule-")
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(DriverError::failed(
                    "capsule.reconcile-invalid",
                    "The capsule root contains an unexpected entry.",
                ));
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                DriverError::failed(
                    "capsule.reconcile-failed",
                    format!("stale capsule metadata: {error}"),
                )
            })?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(DriverError::failed(
                    "capsule.reconcile-invalid",
                    "A stale capsule is not a private directory.",
                ));
            }
            std::fs::remove_dir_all(&path).map_err(|error| {
                DriverError::failed(
                    "capsule.reconcile-failed",
                    format!("stale capsule cleanup: {error}"),
                )
            })?;
            if path.try_exists().map_err(|error| {
                DriverError::failed(
                    "capsule.reconcile-failed",
                    format!("stale capsule absence: {error}"),
                )
            })? {
                return Err(DriverError::failed(
                    "capsule.reconcile-failed",
                    "A stale capsule could not be proven absent.",
                ));
            }
        }
        Ok(())
    }

    fn reconcile_apertures(&self, process_trees_reconciled: bool) -> Result<(), DriverError> {
        let root = self.config.aperture_root();
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(DriverError::failed(
                    "aperture.reconcile-failed",
                    format!("aperture root: {error}"),
                ));
            }
        };
        for (index, entry) in entries.enumerate() {
            if !process_trees_reconciled {
                return Err(DriverError::failed(
                    "aperture.reconcile-unproven",
                    "Stale aperture cleanup requires successful cgroup-root reconciliation.",
                ));
            }
            if index >= self.config.max_tracked_execs {
                return Err(DriverError::failed(
                    "aperture.reconcile-limit",
                    "Stale aperture count exceeds the configured tracked-exec bound.",
                ));
            }
            let entry = entry.map_err(|error| {
                DriverError::failed(
                    "aperture.reconcile-failed",
                    format!("aperture entry: {error}"),
                )
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(DriverError::failed(
                    "aperture.reconcile-invalid",
                    "The aperture root contains a non-UTF-8 entry.",
                ));
            };
            if !name.starts_with("aperture-")
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(DriverError::failed(
                    "aperture.reconcile-invalid",
                    "The aperture root contains an unexpected entry.",
                ));
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                DriverError::failed(
                    "aperture.reconcile-failed",
                    format!("stale aperture metadata: {error}"),
                )
            })?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(DriverError::failed(
                    "aperture.reconcile-invalid",
                    "A stale aperture is not a private directory.",
                ));
            }
            std::fs::remove_dir_all(&path).map_err(|error| {
                DriverError::failed(
                    "aperture.reconcile-failed",
                    format!("stale aperture cleanup: {error}"),
                )
            })?;
        }
        Ok(())
    }

    pub async fn start(
        &self,
        id: &str,
        workspace: &Path,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.start_inner(id, workspace, input, None).await
    }

    /// Starts a confined process with bounded live stdin/stdout/stderr pipes.
    pub async fn start_pipe(
        &self,
        id: &str,
        workspace: &Path,
        input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        // **The mode gate is outermost, and nothing precedes it at either entry point.** That is
        // the whole ordering rule, and it is a rule rather than a preference because the two ports
        // are two implementations of one contract: a request that earns several refusals must not
        // be told a different one by each. `0.9.0` states it in
        // `vectors/http/pty-session-unserved-outranks-a-missing-window.json` and in the
        // `session.pty-refusal-order` coverage requirement.
        //
        // Four checks exist at both ports. Their relative order agrees member for member —
        // `pty fact` < `window shape` < {`wait`, `session bounds`} — which is what
        // `the_two_entry_points_name_one_refusal_for_a_pty_start_that_earns_two` and
        // `the_overlapping_checks_rank_the_same_at_both_entry_points` assert. Everything after
        // those four is each port's own business: the daemon owns request shape, this owns host
        // capability, and the daemon is always reached first, so a driver refusal is only ever
        // visible for something the daemon admitted.
        //
        // `session.window-invalid` and `session.wait-invalid` both invite a client on a
        // terminal-less deployment to change the request and retry into a refusal it can never get
        // past; `session.pty-unserved` says stop. In no case is a pipe session started instead
        // (design 13, invariant 3).
        if input.mode == SessionMode::Pty && self.capability.facts.sessions_pty != Some(true) {
            return DispatchOutcome::NotDispatched(DriverError::unserved(
                substrate_wire::SESSION_PTY_UNSERVED,
                "This driver did not prove it can give a confined process a controlling terminal.",
                "mode",
            ));
        }
        if substrate_wire::validate_session_window(input.mode, input.window.as_ref()).is_err() {
            return DispatchOutcome::NotDispatched(DriverError::refused(
                substrate_wire::SESSION_WINDOW_INVALID,
                "A pty session declares an initial window within the closed cell bounds, and a raw-pipe session declares none.",
                "window",
            ));
        }
        // Rank 3, to match the daemon. The driver used to reach this only inside `start_inner`,
        // after the exec shape, so the two ports ranked the confinement floor and the exec shape in
        // opposite orders — the same defect as the `wait` pair, one member over.
        if self.config.cgroup_root.is_none() {
            return DispatchOutcome::NotDispatched(sandbox_unavailable());
        }
        if input.exec.wait {
            return DispatchOutcome::NotDispatched(DriverError::refused(
                "session.wait-invalid",
                "A raw-pipe session cannot use synchronous exec wait.",
                "wait",
            ));
        }
        if input.input_limit_bytes == 0
            || input.frame_limit_bytes == 0
            || input.frame_limit_bytes > PIPE_FRAME_BYTES as u64
            || input.queued_frames == 0
            || input.queued_frames
                > u32::try_from(PIPE_QUEUED_FRAMES).expect("queue ceiling fits u32")
        {
            return DispatchOutcome::NotDispatched(DriverError::exhausted(
                "session.limit-unserved",
                "Raw-pipe bounds exceed the host development profile.",
                "session",
            ));
        }
        let settings = PipeSettings {
            input_limit: input.input_limit_bytes,
            frame_limit: usize::try_from(input.frame_limit_bytes)
                .expect("bounded frame fits usize"),
            queued_frames: usize::try_from(input.queued_frames).expect("bounded queue fits usize"),
            window: match input.mode {
                SessionMode::Pipes => None,
                SessionMode::Pty => input.window,
            },
        };
        self.start_inner(id, workspace, &input.exec, Some(settings))
            .await
    }

    #[allow(clippy::too_many_lines)] // Admission, spawn barrier, and durable observation stay adjacent.
    async fn start_inner(
        &self,
        id: &str,
        workspace: &Path,
        input: &ExecStartInput,
        pipe_settings: Option<PipeSettings>,
    ) -> DispatchOutcome<ExecObservation> {
        if let Err(error) = self.admit(id, workspace, input) {
            return DispatchOutcome::NotDispatched(error);
        }
        let capsule = match self.prepare_capsule(input.capsule.as_ref()) {
            Ok(value) => value,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        let permit =
            match ActivePermit::acquire(Arc::clone(&self.active), self.config.max_concurrent_execs)
            {
                Ok(value) => value,
                Err(error) => return DispatchOutcome::NotDispatched(error),
            };
        let tracking = match TrackingReservation::acquire(
            Arc::clone(&self.executions),
            Arc::clone(&self.reservations),
            id,
            self.config.max_tracked_execs,
        ) {
            Ok(value) => value,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        let Some(cgroup_root) = self.config.cgroup_root.as_deref() else {
            return DispatchOutcome::NotDispatched(sandbox_unavailable());
        };
        let cgroup = match Cgroup::create(cgroup_root, id, input) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        // Selected again rather than carried out of `admit`, because it is a pure function of the
        // request and the configuration and re-asking is cheaper than threading a borrow through
        // every early return above.
        let aperture = match Self::admit_egress_aperture(
            &self.config,
            self.capability.facts.exec_egress_apertures.as_ref(),
            &input.sandbox,
        ) {
            Ok(value) => value.cloned(),
            Err(error) => return contain_cgroup(&cgroup, error),
        };
        // Generated before the spawn because bubblewrap binds it, and installed after, because the
        // namespace it points into does not exist yet.
        let resolution = match aperture.as_ref() {
            Some(aperture) => match crate::egress::GeneratedResolution::prepare(
                aperture,
                self.config.ca_bundle.as_deref(),
                &self.config.aperture_root(),
            ) {
                Ok(value) => Some(value),
                Err(error) => return contain_cgroup(&cgroup, error),
            },
            None => None,
        };
        let (sync_read, sync_write) = match pipe() {
            Ok(pipe) => pipe,
            Err(error) => {
                return contain_cgroup(&cgroup, error);
            }
        };
        let sync_fd = sync_read.as_raw_fd();
        // Trap 2: the namespace to bind the aperture in belongs to the pid bubblewrap *reports*,
        // never to the bubblewrap process itself.
        let info = match aperture.as_ref() {
            Some(_) => match pipe() {
                Ok(pipe) => Some(pipe),
                Err(error) => return contain_cgroup(&cgroup, error),
            },
            None => None,
        };
        let info_fd = info.as_ref().map(|(_, write)| write.as_raw_fd());
        // Last thing before spawn, first thing released after it. Every admission check and the
        // backend recheck are behind us, so a start that will not run never reads a credential
        // (`docs/design/11-sealed-secret-slots.md` § 4).
        let slots = match crate::secrets::SecretSlotSet::acquire(
            &self.config.secret_slots,
            &input.secret_slots,
        ) {
            Ok(slots) => slots,
            Err(error) => return contain_cgroup(&cgroup, error),
        };
        // Allocated last, so a start that will not run never takes a pty off the host's global
        // count. Failure is `exhausted` and retriable: that count is a resource other tenants can
        // fill and free, which is not the same claim as "this host cannot do terminals".
        let terminal = match pipe_settings.and_then(|settings| settings.window) {
            Some(window) => match crate::pty::open(window) {
                Ok(pair) => Some(pair),
                Err(error) => {
                    return contain_cgroup(
                        &cgroup,
                        DriverError::exhausted(
                            substrate_wire::SESSION_PTY_EXHAUSTED,
                            format!("pty allocation: {error}"),
                            "session",
                        ),
                    );
                }
            },
            None => None,
        };
        let channel = match terminal.as_ref() {
            Some(pair) => match child_terminal_descriptors(pair) {
                Ok(descriptors) => ChildChannel::Terminal(descriptors),
                Err(error) => return contain_cgroup(&cgroup, error),
            },
            None if pipe_settings.is_some() => ChildChannel::Pipes,
            None => ChildChannel::Quiet,
        };
        let (mut command, seccomp) = match self.command(
            workspace,
            input,
            sync_fd,
            channel,
            capsule.as_ref(),
            &slots,
            resolution.as_ref(),
            info_fd,
        ) {
            Ok(value) => value,
            Err(error) => return contain_cgroup(&cgroup, error),
        };
        let write_fd = sync_write.as_raw_fd();
        let placements = slots.placements();
        let mut retained = slots.retained(Some(sync_fd));
        if let Some(fd) = info_fd.and_then(|fd| u32::try_from(fd).ok()) {
            retained.push(fd);
            retained.sort_unstable();
            retained.dedup();
        }
        retained.push(u32::try_from(seccomp.as_raw_fd()).expect("seccomp descriptor fits u32"));
        retained.sort_unstable();
        retained.dedup();
        // SAFETY: pre_exec runs after fork; it invokes only async-signal-safe libc calls and does
        // not allocate. The captured descriptors are plain integers owned by the parent until spawn,
        // and the two vectors are built before the fork and only read after it.
        unsafe {
            command.as_std_mut().pre_exec(move || {
                if libc::close(write_fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                crate::secrets::place_and_close(&placements, &retained)?;
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let spawned = command.spawn();
        // The parent's copies of the terminal go with the builder: while any of them is open the
        // master never sees the child's exit, so a session that ended would read as one that stalled.
        drop(command);
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                return contain_cgroup(
                    &cgroup,
                    DriverError::failed(
                        "exec.spawn-failed",
                        format!("bubblewrap spawn failed: {error}"),
                    ),
                );
            }
        };
        drop(sync_read);
        let terminal = match terminal {
            Some(pair) => {
                drop(pair.slave);
                match crate::pty::PtyMaster::new(pair.master) {
                    Ok(master) => Some(Arc::new(master)),
                    Err(error) => {
                        return contain_spawned(
                            child,
                            cgroup,
                            DriverError::failed(
                                substrate_wire::SESSION_PTY_FAILED,
                                format!("pty master: {error}"),
                            ),
                        )
                        .await;
                    }
                }
            }
            None => None,
        };
        // The child now holds the only copy. A memfd has no name in any filesystem, so this is the
        // whole release: when the last descriptor goes, the kernel frees the memory.
        let applied_slots = slots.applied();
        drop(slots);
        let Some(leader_pid) = child.id() else {
            let error =
                DriverError::failed("exec.spawn-failed", "spawn returned no process identity");
            return contain_spawned(child, cgroup, error).await;
        };
        if let Err(error) = cgroup.attach_tree(leader_pid) {
            return contain_spawned(child, cgroup, error).await;
        }
        // Between the namespace existing and the child running: the aperture is there for every
        // instruction the child executes, and for none before it (ADR 0013).
        let installed = match (aperture.as_ref(), info) {
            (Some(aperture), Some((info_read, info_write))) => {
                drop(info_write);
                let mut reader = std::fs::File::from(info_read);
                let Some(sandbox_pid) = crate::egress::read_sandbox_pid(&mut reader) else {
                    return contain_spawned(
                        child,
                        cgroup,
                        DriverError::failed(
                            "exec.aperture-install-failed",
                            "The confinement backend reported no sandbox process identity.",
                        ),
                    )
                    .await;
                };
                match crate::egress::install(aperture, sandbox_pid, Some(&cgroup.procs())) {
                    Ok(installed) => Some(installed),
                    // Nothing partial: the aperture either exists exactly as declared or the
                    // dispatch is refused with the sandbox torn down around it.
                    Err(error) => return contain_spawned(child, cgroup, error).await,
                }
            }
            _ => None,
        };
        if let Err(error) = release_barrier(&sync_write) {
            return contain_spawned(child, cgroup, error).await;
        }
        drop(sync_write);

        let applied_capsule = capsule.as_ref().map(|value| value.applied.clone());
        let applied = AppliedConfinement {
            capability_snapshot: self.capability.snapshot.clone(),
            cgroup: cgroup.name().to_owned(),
            filesystem: if applied_capsule.is_some() {
                AppliedFilesystem::WorkspaceReadWriteCapsuleReadOnlySystemReadOnly
            } else {
                AppliedFilesystem::WorkspaceReadWriteSystemReadOnly
            },
            // Reported, never inferred: what is here is what the forwarder was actually given,
            // and the destination is the pinned address rather than the configured host string.
            network: installed
                .as_ref()
                .map_or(AppliedNetwork::None, |installed| {
                    AppliedNetwork::Aperture(installed.applied())
                }),
            profile: SandboxProfile::Workspace,
            capsule: applied_capsule,
            // What was mounted, not what was asked for: `admit` refused anything it could not
            // mount exactly as declared, so by here the two are the same list — and a reader gets
            // it from the record rather than from the fact that a build happened to succeed.
            read_only_roots: input.read_only_roots.clone(),
            // Placed, not requested: this list is taken from the acquired set, so it names what
            // actually crossed into the child.
            secret_slots: applied_slots,
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
            lease: None,
            refusal: None,
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
        let (pipe_sender, pipe) = if let Some(settings) = pipe_settings {
            let stdin = if terminal.is_some() {
                None
            } else if let Some(stdin) = child.stdin.take() {
                Some(stdin)
            } else {
                let error = DriverError::failed(
                    "session.stdin-missing",
                    "Raw-pipe process did not expose stdin.",
                );
                return contain_spawned(child, cgroup, error).await;
            };
            let (sender, receiver) = mpsc::channel(settings.queued_frames);
            (
                Some(sender),
                Some(PipeState {
                    stdin: tokio::sync::Mutex::new(stdin),
                    output: tokio::sync::Mutex::new(receiver),
                    input_bytes: AtomicU64::new(0),
                    input_limit: settings.input_limit,
                    frame_limit: settings.frame_limit,
                    terminal: terminal.clone(),
                }),
            )
        } else {
            (None, None)
        };
        let output_bound = Arc::new(AtomicBool::new(false));
        let execution = Arc::new(Execution::new(observation.clone(), pipe));
        tracking.install(Arc::clone(&execution));
        let timeout = Duration::from_millis(input.limits.timeout_ms);
        let cpu_budget_micros = input.limits.cpu_millis.saturating_mul(1_000);
        let output_limit = usize::try_from(input.limits.output_bytes)
            .unwrap_or(usize::MAX)
            .min(usize::try_from(self.config.output_limit_bytes).unwrap_or(usize::MAX));
        tokio::spawn(run_child(
            child,
            cgroup,
            Arc::clone(&execution),
            timeout,
            cpu_budget_micros,
            output_limit,
            pipe_sender,
            pipe_settings.map_or(PIPE_FRAME_BYTES, |settings| settings.frame_limit),
            capsule,
            installed,
            resolution,
            permit,
            terminal,
            output_bound,
        ));
        if input.wait {
            if let Err(error) =
                wait_terminal(&execution, timeout.saturating_add(Duration::from_secs(5))).await
            {
                return DispatchOutcome::OutcomeUnknown(error);
            }
            DispatchOutcome::Observed(execution.observation.lock().clone())
        } else {
            DispatchOutcome::Observed(observation)
        }
    }

    pub async fn write_pipe(&self, id: &str, bytes: &[u8]) -> Result<(), DriverError> {
        let execution = self.execution(id)?;
        let pipe = execution.pipe.as_ref().ok_or_else(|| {
            DriverError::refused(
                "session.not-pipe",
                "Exec is not a raw-pipe session.",
                "session",
            )
        })?;
        if bytes.is_empty() || bytes.len() > pipe.frame_limit {
            return Err(DriverError::exhausted(
                "session.frame-limit",
                "Raw-pipe input frame is outside the admitted bounds.",
                "frame",
            ));
        }
        let count = u64::try_from(bytes.len()).expect("usize fits u64");
        let admitted =
            pipe.input_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current
                        .checked_add(count)
                        .filter(|next| *next <= pipe.input_limit)
                });
        if admitted.is_err() {
            return Err(DriverError::exhausted(
                "session.input-limit",
                "Raw-pipe input exceeds the admitted byte limit.",
                "stdin",
            ));
        }
        if let Some(terminal) = pipe.terminal.as_ref() {
            // The same guard `resize_pty` carries, for the same reason: the master outlives the
            // child, so its input queue keeps accepting bytes with no slave left to read them.
            // Reporting that as delivered tells a client its input reached a process that has
            // finished. The raw-pipe branch below gets this for free — its descriptor is gone — and
            // a terminal has to be asked.
            if is_terminal(execution.observation.lock().resource.state) {
                return Err(DriverError::refused(
                    substrate_wire::SESSION_PTY_ENDED,
                    "The pty session has ended; its child already finished, so nothing can read this input.",
                    "session",
                ));
            }
            // Straight at the master: the line discipline between it and the child is what turns
            // these bytes into what the child reads, and into the echo the client sees back.
            return terminal.write_all(bytes).await.map_err(|error| {
                DriverError::failed(
                    substrate_wire::SESSION_WRITE_FAILED,
                    format!("pty input: {error}"),
                )
            });
        }
        let mut stdin = pipe.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(DriverError::refused(
                "session.input-closed",
                "Raw-pipe stdin is already closed.",
                "stdin",
            ));
        };
        stdin.write_all(bytes).await.map_err(|error| {
            DriverError::failed("session.write-failed", format!("raw-pipe stdin: {error}"))
        })?;
        stdin.flush().await.map_err(|error| {
            DriverError::failed("session.write-failed", format!("raw-pipe stdin: {error}"))
        })
    }

    pub async fn close_pipe_input(&self, id: &str) -> Result<(), DriverError> {
        let execution = self.execution(id)?;
        let pipe = execution.pipe.as_ref().ok_or_else(|| {
            DriverError::refused(
                "session.not-pipe",
                "Exec is not a raw-pipe session.",
                "session",
            )
        })?;
        if pipe.terminal.is_some() {
            // A pty has no half-close (design 13). A client ends input by sending the terminal's
            // own end-of-file character as ordinary input bytes, which is line-discipline
            // behaviour and not a frame.
            return Err(DriverError::refused(
                substrate_wire::SESSION_INPUT_CLOSE_UNSERVED,
                "A pty session has no half-close; send the terminal's own end-of-file character as input.",
                "session",
            ));
        }
        pipe.stdin.lock().await.take();
        Ok(())
    }

    /// Applies a new window to a live terminal.
    ///
    /// The kernel signals the foreground process group; the child observes the change with
    /// `TIOCGWINSZ` the way any process does. Nothing is injected into the environment: `COLUMNS`
    /// and `LINES` would go stale at exactly this call (design 13).
    ///
    /// Both guards are here rather than only in the daemon, because this is the driver port and a
    /// port is a contract. The cell bound is a bound on the *terminal*, and the terminal lives
    /// here: `TIOCSWINSZ` takes an `unsigned short`, so a caller that skipped the frame decoder
    /// could hand the child a 65535x65535 window, which is not a display but an amplification knob
    /// — programs allocate per-cell buffers when the size changes, and that allocation is spent
    /// from the run's own memory bound (`substrate_wire::MAX_PTY_WINDOW_COLUMNS`). And the master
    /// descriptor outlives the child, so the ioctl succeeds long after anything can observe it;
    /// reporting that as applied would tell a client a child saw a window no process will ever
    /// read.
    ///
    /// # Errors
    ///
    /// Returns [`substrate_wire::SESSION_RESIZE_INVALID`] for a window outside the closed cell
    /// bounds, [`substrate_wire::SESSION_NOT_PTY`] when the exec is not a pty session at all,
    /// [`substrate_wire::SESSION_PTY_ENDED`] when it is one whose child has finished, and
    /// [`substrate_wire::SESSION_RESIZE_FAILED`] when the kernel refuses the resize.
    pub fn resize_pty(&self, id: &str, window: PtyWindow) -> Result<(), DriverError> {
        if substrate_wire::validate_session_window(SessionMode::Pty, Some(&window)).is_err() {
            return Err(DriverError::refused(
                substrate_wire::SESSION_RESIZE_INVALID,
                "A resize names 1 to 1000 cells on each axis of a pty session.",
                "window",
            ));
        }
        let execution = self.execution(id)?;
        let terminal = execution
            .pipe
            .as_ref()
            .and_then(|pipe| pipe.terminal.as_ref())
            .ok_or_else(|| {
                DriverError::refused(
                    substrate_wire::SESSION_NOT_PTY,
                    "Exec is not a pty session.",
                    "session",
                )
            })?;
        // Its own code, because it is its own condition: this exec *is* a pty session, and what is
        // wrong is that nothing is left to observe the new window. `session.not-pty` next door means
        // the caller named the wrong kind of thing, which is a different mistake with a different
        // fix.
        if is_terminal(execution.observation.lock().resource.state) {
            return Err(DriverError::refused(
                substrate_wire::SESSION_PTY_ENDED,
                "The pty session has ended; its child already finished, so no process can observe a new window.",
                "session",
            ));
        }
        terminal.resize(window).map_err(|error| {
            DriverError::failed(
                substrate_wire::SESSION_RESIZE_FAILED,
                format!("pty resize: {error}"),
            )
        })
    }

    pub async fn read_pipe(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        if timeout.is_zero() {
            return Err(DriverError::refused(
                "session.timeout-invalid",
                "Raw-pipe read timeout must be nonzero.",
                "timeout",
            ));
        }
        let execution = self.execution(id)?;
        let pipe = execution.pipe.as_ref().ok_or_else(|| {
            DriverError::refused(
                "session.not-pipe",
                "Exec is not a raw-pipe session.",
                "session",
            )
        })?;
        let mut output = pipe.output.lock().await;
        let frame = tokio::time::timeout(timeout, output.recv())
            .await
            .map_err(|_| {
                DriverError::failed("session.read-timeout", "Raw-pipe read deadline elapsed.")
            })?;
        drop(output);
        if frame.is_none() {
            // The output senders close just before the child task records its cgroup-reconciled
            // terminal observation. Never let an attachment turn that narrow ordering window into
            // a premature EOF claim.
            wait_terminal(&execution, Duration::from_secs(5)).await?;
        }
        Ok(frame)
    }

    fn execution(&self, id: &str) -> Result<Arc<Execution>, DriverError> {
        self.executions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(DriverError::not_found)
    }

    pub fn observe(&self, id: &str) -> Result<ExecObservation, DriverError> {
        let execution = self
            .executions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(DriverError::not_found)?;
        let observation = execution.observation.lock().clone();
        Ok(observation)
    }

    pub fn acknowledge(&self, persisted: &ExecObservation) {
        if !is_terminal(persisted.resource.state) {
            return;
        }
        let mut executions = self.executions.lock();
        let exact_terminal_match = executions
            .get(&persisted.resource.id)
            .is_some_and(|execution| *execution.observation.lock() == *persisted);
        if exact_terminal_match {
            executions.remove(&persisted.resource.id);
        }
    }

    pub fn discard_terminal(&self, id: &str) {
        let mut executions = self.executions.lock();
        if executions
            .get(id)
            .is_some_and(|execution| is_terminal(execution.observation.lock().resource.state))
        {
            executions.remove(id);
        }
    }

    pub fn completed(&self) -> Vec<ExecObservation> {
        self.executions
            .lock()
            .values()
            .filter_map(|execution| {
                let observation = execution.observation.lock();
                is_terminal(observation.resource.state).then(|| observation.clone())
            })
            .collect()
    }

    pub fn set_lease(&self, id: &str, lease: Option<substrate_wire::LeaseObservation>) {
        if let Some(execution) = self.executions.lock().get(id) {
            execution.observation.lock().resource.lease = lease;
        }
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
        if let Some(pipe) = &execution.pipe {
            // A disconnected or backpressured attachment may no longer drain the bounded output
            // queue. Close its receiver before signalling so output pumps can finish draining the
            // kernel pipes and terminal reconciliation cannot deadlock behind a full queue.
            pipe.output.lock().await.close();
        }
        let cgroup_name = execution
            .observation
            .lock()
            .cgroup
            .clone()
            .ok_or_else(|| DriverError::failed("exec.cgroup-missing", "Exec has no cgroup"))?;
        let cgroup_root = self
            .config
            .cgroup_root
            .as_deref()
            .ok_or_else(sandbox_unavailable)?;
        let cgroup = Cgroup::existing(cgroup_root, &cgroup_name)?;
        match input.signal {
            Signal::Kill => {
                cgroup.kill_all()?;
                execution
                    .cancellation_requested
                    .store(true, Ordering::Release);
                *execution.delivered_signal.lock() = Some(Signal::Kill);
            }
            signal => {
                let leader = execution.observation.lock().leader_pid;
                cgroup.signal_all(signal, leader)?;
                *execution.delivered_signal.lock() = Some(signal);
                let grace = Duration::from_millis(input.grace_ms);
                if wait_terminal(&execution, grace).await.is_err() {
                    cgroup.kill_all()?;
                    execution
                        .cancellation_requested
                        .store(true, Ordering::Release);
                    *execution.delivered_signal.lock() = Some(Signal::Kill);
                }
            }
        }
        wait_terminal(&execution, Duration::from_secs(5)).await?;
        let observation = execution.observation.lock().clone();
        Ok(observation)
    }

    /// Every rule ADR 0010 names, checked before dispatch.
    ///
    /// Textual rules first, because they are the caller's mistake to hear about; then the
    /// filesystem question only the driver can answer without racing.
    fn admit_read_only_roots(roots: &[substrate_wire::ReadOnlyRoot]) -> Result<(), DriverError> {
        substrate_wire::validate_read_only_roots(roots).map_err(|error| {
            DriverError::refused(
                "exec.read-only-root-invalid",
                format!("A declared read-only root is invalid: {error}"),
                "read_only_roots",
            )
        })?;
        for root in roots {
            // `symlink_metadata`, so a symlink is refused as itself rather than followed to
            // whatever it points at — the caller declared a directory and would not be told.
            let metadata = std::fs::symlink_metadata(std::path::Path::new(&root.host_path))
                .map_err(|_| {
                    DriverError::refused(
                        "exec.read-only-root-absent",
                        "A declared read-only root is not present on this host.",
                        "read_only_roots",
                    )
                })?;
            if !metadata.is_dir() {
                return Err(DriverError::refused(
                    "exec.read-only-root-not-a-directory",
                    "A declared read-only root is not a directory.",
                    "read_only_roots",
                ));
            }
        }
        Ok(())
    }

    /// Every rule ADR 0012 names about *which* slots a start may ask for, before anything is read.
    ///
    /// Order matters. Shape first, because it is the caller's mistake and does not depend on this
    /// host; then the capability, because a slot asked for where none can be delivered is
    /// `unserved` and never a weaker delivery; then whether the operator declared this name.
    pub(crate) fn admit_secret_slots(
        config: &HostConfig,
        published: Option<&Vec<String>>,
        requested: &[substrate_wire::SecretSlotRequest],
    ) -> Result<(), DriverError> {
        if requested.is_empty() {
            return Ok(());
        }
        substrate_wire::validate_secret_slots(requested).map_err(|error| {
            DriverError::refused(
                "exec.secret-slot-descriptor-invalid",
                format!("A named secret slot is not admissible: {error}"),
                "secret_slots",
            )
        })?;
        if published.is_none() {
            return Err(DriverError::unserved(
                "exec.secret-slots-unserved",
                "Sealed secret slots are not served by this host.",
                "secret_slots",
            ));
        }
        for slot in requested {
            if !config
                .secret_slots
                .iter()
                .any(|declaration| declaration.name == slot.slot)
            {
                return Err(DriverError::refused(
                    "exec.secret-slot-unknown",
                    format!("Secret slot {} is not declared on this host.", slot.slot),
                    "secret_slots",
                ));
            }
        }
        Ok(())
    }

    /// Which declared aperture this start selected, if any, and why it may not have one
    /// (ADR 0013).
    ///
    /// Shape, then capability, then declaration — the order `admit_secret_slots` uses, so a caller
    /// hears the same answer whichever layer sees the request first. Nothing here reads a
    /// destination out of the request, because there is no destination in a request: a name is the
    /// only thing a caller may say, and configuration owns everything the name stands for.
    pub(crate) fn admit_egress_aperture<'a>(
        config: &'a HostConfig,
        published: Option<&Vec<substrate_wire::EgressApertureFact>>,
        sandbox: &substrate_wire::ConfinementRequest,
    ) -> Result<Option<&'a crate::egress::EgressAperture>, DriverError> {
        substrate_wire::validate_aperture_request(sandbox.network, sandbox.aperture.as_deref())
            .map_err(|error| match error {
                // A *name* that parses as a destination is a rejected escalation, not a typo, and
                // saying so is the difference between "you may not" and "no such thing".
                substrate_wire::WireValidationError::ApertureDestinationInRequest => {
                    DriverError::refused(
                        "exec.aperture-destination-in-request",
                        "An egress aperture is selected by name; a destination may not appear in a request.",
                        "sandbox.network.aperture",
                    )
                }
                // A ceiling is declared where reach is declared. Named apart from the destination
                // refusal so a rejected escalation reads as one and not as a typo (ADR 0014).
                substrate_wire::WireValidationError::ApertureCeilingInRequest => {
                    DriverError::refused(
                        "exec.aperture-ceiling-in-request",
                        "An egress aperture byte ceiling is declared by the operator; it may not appear in a request.",
                        "sandbox.network.aperture",
                    )
                }
                substrate_wire::WireValidationError::InvalidApertureName => DriverError::refused(
                    "exec.aperture-name-invalid",
                    "An egress aperture name must match [a-z][a-z0-9_]{0,63}.",
                    "sandbox.network.aperture",
                ),
                _ => DriverError::refused(
                    "exec.aperture-mode-mismatch",
                    "A sandbox asks for network \"aperture\" with a name, or \"none\" without one.",
                    "sandbox.network",
                ),
            })?;
        let Some(name) = sandbox.aperture.as_deref() else {
            return Ok(None);
        };
        // Configured intent is not a fact. An unverified mechanism leaves the capability absent,
        // and every aperture request is then `unserved` rather than quietly getting no network.
        let Some(published) = published else {
            return Err(DriverError::unserved(
                "exec.egress-apertures-unserved",
                "Egress apertures are not served by this host.",
                "exec.network-aperture",
            ));
        };
        let declared = published
            .iter()
            .any(|fact| fact.name == name)
            .then(|| {
                config
                    .egress_apertures
                    .iter()
                    .find(|aperture| aperture.name == name)
            })
            .flatten();
        declared.map(Some).ok_or_else(|| {
            DriverError::unserved(
                // The name, because an operator debugging a harness needs to know which one was
                // asked for, and a name is deployment vocabulary rather than secret material.
                "exec.aperture-undeclared",
                format!("Egress aperture {name} is not declared on this host."),
                "exec.network-aperture",
            )
        })
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
        Self::admit_read_only_roots(&input.read_only_roots)?;
        Self::admit_secret_slots(
            &self.config,
            self.capability.facts.secrets_slots.as_ref(),
            &input.secret_slots,
        )?;
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
        Self::admit_egress_aperture(
            &self.config,
            self.capability.facts.exec_egress_apertures.as_ref(),
            &input.sandbox,
        )?;
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
        if let Some(capsule) = &input.capsule {
            let expected = format!("{EXECUTION_CAPSULE_MOUNT}/{}", capsule.entrypoint);
            if input.argv.first() != Some(&expected) {
                return Err(DriverError::refused(
                    "capsule.entrypoint-mismatch",
                    "Exec argv does not start with the declared capsule entrypoint.",
                    "argv",
                ));
            }
        }
        if input.limits.timeout_ms == 0
            || input.limits.timeout_ms > 86_400_000
            || input.limits.output_bytes == 0
            || input.limits.output_bytes > self.config.output_limit_bytes
            || input.limits.processes == 0
            || input.limits.processes > 4096
            || input.limits.memory_bytes < 1_048_576
            || input.limits.memory_bytes > 1_099_511_627_776
            || input.limits.cpu_millis == 0
            || input.limits.cpu_millis > 86_400_000
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
        if self.backend_binding.is_none() {
            return Err(sandbox_unavailable());
        }
        let current = crate::probe::backend_binding(&self.config);
        if current.is_none() || current != self.backend_binding {
            return Err(DriverError::refused(
                "exec.capability-stale",
                "The admitted confinement backend identity or configuration changed after probing.",
                "capability_snapshot",
            ));
        }
        Ok(())
    }

    // Nine, because nine separate things shape one argv and a struct of them would be a second
    // name for this function's parameter list.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)] // One argv, built in one place, in the order bwrap reads it.
    fn command(
        &self,
        workspace: &Path,
        input: &ExecStartInput,
        sync_fd: RawFd,
        channel: ChildChannel,
        capsule: Option<&PreparedCapsule>,
        slots: &crate::secrets::SecretSlotSet,
        resolution: Option<&crate::egress::GeneratedResolution>,
        info_fd: Option<RawFd>,
    ) -> Result<(Command, File), DriverError> {
        let seccomp = crate::seccomp::profile()?;
        let seccomp_fd = seccomp.as_raw_fd();
        let mut command = Command::new(&self.config.bubblewrap);
        let mut terminal = false;
        command.env_clear().kill_on_drop(true);
        match channel {
            ChildChannel::Quiet => {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
            }
            ChildChannel::Pipes => {
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
            }
            ChildChannel::Terminal(descriptors) => {
                let [input, output, diagnostics] = *descriptors;
                command
                    .stdin(Stdio::from(input))
                    .stdout(Stdio::from(output))
                    .stderr(Stdio::from(diagnostics));
                terminal = true;
            }
        }
        command.args([
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
        ]);
        command.arg("--seccomp").arg(seccomp_fd.to_string());
        if let Some(capsule) = capsule {
            command
                .arg("--ro-bind")
                .arg(capsule.directory.path())
                .arg(EXECUTION_CAPSULE_MOUNT);
        }
        // `--ro-bind` and never `--bind`: a declared root is readable and never writable, so a
        // process cannot alter the toolchain it was given any more than it can alter `/usr`, and
        // the workspace stays the only writable path. Before the workspace bind, so a root can
        // never shadow it — though `admit` has already refused that mount point by name.
        for root in &input.read_only_roots {
            command
                .arg("--ro-bind")
                .arg(&root.host_path)
                .arg(&root.mount);
        }
        // The whole of `/etc` a run ever gets, and only when it has an aperture: exactly the
        // declared name mapped to the forwarder, and — where the operator configured one — a
        // snapshot of a trust anchor. Read-only, generated, and gone with the run.
        if let Some(resolution) = resolution {
            for (source, mount) in resolution.binds() {
                command.arg("--ro-bind").arg(source).arg(mount);
            }
        }
        command
            .arg("--bind")
            .arg(workspace)
            .args(["/workspace", "--chdir", "/workspace", "--block-fd"])
            .arg(sync_fd.to_string());
        if let Some(info_fd) = info_fd {
            command.arg("--info-fd").arg(info_fd.to_string());
        }
        for name in &input.env.allow {
            command.args(["--setenv", name.as_str(), baseline_value(*name)]);
        }
        for (name, value) in &input.env.set {
            command.args(["--setenv", name, value]);
        }
        // The mapping and nothing else: names and descriptors, sorted, ASCII. Absent when this
        // start named no slot, so a child can tell "no slots" from "an empty list".
        if let Some(mapping) = slots.environment() {
            command.args(["--setenv", substrate_wire::SECRET_SLOTS_ENV, &mapping]);
        }
        // Where the generated anchor is, because the conventional path is not a convention every
        // TLS stack follows. Absent when no anchor was generated: a child that cannot verify must
        // find out by failing to verify, never by being pointed at an empty file.
        if resolution.is_some_and(crate::egress::GeneratedResolution::has_ca_bundle) {
            command.args([
                "--setenv",
                "SSL_CERT_FILE",
                substrate_wire::APERTURE_CA_BUNDLE_PATH,
            ]);
        }
        command.arg("--");
        // Design 13: after bubblewrap's own `setsid`, and never instead of it. `--new-session`
        // stays on every exec; this takes the inherited slave as the controlling terminal from
        // inside the sandbox, which is what gives the child a foreground process group — and so a
        // `SIGWINCH` on a resize and a hangup when the master closes.
        if terminal {
            command.args(crate::pty::CONTROLLING_TERMINAL_ARGV);
        }
        // Bubblewrap injects PWD after `--clearenv`; remove that implementation detail before
        // exec, then restore it only when the closed request explicitly supplied PWD.
        command.args(["/usr/bin/env", "-u", "PWD", "--"]);
        if let Some(value) = input.env.set.get("PWD") {
            command.args(["/usr/bin/env", "--"]);
            command.arg(format!("PWD={value}"));
        }
        command.args(&input.argv);
        Ok((command, seccomp))
    }

    fn prepare_capsule(
        &self,
        capsule: Option<&ExecutionCapsuleInput>,
    ) -> Result<Option<PreparedCapsule>, DriverError> {
        let Some(capsule) = capsule else {
            return Ok(None);
        };
        let validation = substrate_wire::validate_execution_capsule(capsule).map_err(|error| {
            DriverError::refused(
                "capsule.invalid",
                format!("Execution capsule validation failed: {error}"),
                "capsule",
            )
        })?;
        let directory = tempfile::Builder::new()
            .prefix("capsule-")
            .tempdir_in(&self.config.capsule_root)
            .map_err(|error| {
                DriverError::failed(
                    "capsule.materialization-failed",
                    format!("create private capsule: {error}"),
                )
            })?;
        for file in &capsule.files {
            let destination = directory.path().join(&file.path);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    DriverError::failed(
                        "capsule.materialization-failed",
                        format!("create capsule directory: {error}"),
                    )
                })?;
            }
            let bytes = file.content.decode().map_err(|error| {
                DriverError::refused(
                    "capsule.invalid",
                    format!("decode capsule file: {error}"),
                    "capsule.files.content",
                )
            })?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if file.executable { 0o500 } else { 0o400 })
                .open(&destination)
                .map_err(|error| {
                    DriverError::failed(
                        "capsule.materialization-failed",
                        format!("create capsule file: {error}"),
                    )
                })?;
            output.write_all(&bytes).map_err(|error| {
                DriverError::failed(
                    "capsule.materialization-failed",
                    format!("write capsule file: {error}"),
                )
            })?;
            output.sync_all().map_err(|error| {
                DriverError::failed(
                    "capsule.materialization-failed",
                    format!("sync capsule file: {error}"),
                )
            })?;
            std::fs::set_permissions(
                &destination,
                std::fs::Permissions::from_mode(if file.executable { 0o500 } else { 0o400 }),
            )
            .map_err(|error| {
                DriverError::failed(
                    "capsule.materialization-failed",
                    format!("set capsule file mode: {error}"),
                )
            })?;
        }
        Ok(Some(PreparedCapsule {
            directory,
            applied: AppliedExecutionCapsule {
                manifest_sha256: capsule.manifest_sha256.clone(),
                entrypoint: format!("{EXECUTION_CAPSULE_MOUNT}/{}", capsule.entrypoint),
                mount: EXECUTION_CAPSULE_MOUNT.to_owned(),
                file_count: validation.file_count,
                total_bytes: validation.total_bytes,
            },
        }))
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // Drain, wait and terminal observation stay in one audit unit.
async fn run_child(
    mut child: Child,
    cgroup: Cgroup,
    execution: Arc<Execution>,
    timeout: Duration,
    cpu_budget_micros: u64,
    output_limit: usize,
    pipe_sender: Option<mpsc::Sender<PipeFrame>>,
    frame_limit: usize,
    capsule: Option<PreparedCapsule>,
    aperture: Option<crate::egress::InstalledAperture>,
    resolution: Option<crate::egress::GeneratedResolution>,
    _permit: ActivePermit,
    terminal: Option<Arc<crate::pty::PtyMaster>>,
    output_bound: Arc<AtomicBool>,
) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // A terminal has one file. Stdout and stderr *are* the same descriptor on a pty, so the merged
    // stream is captured as `stdout` and the durable stderr is genuinely empty — reported, not
    // inferred (design 13).
    let (stdout_task, stderr_task) = if let Some(master) = terminal {
        let reached = Some(Arc::clone(&output_bound));
        let terminal_execution = pipe_sender.as_ref().map(|_| Arc::clone(&execution));
        let merged = tokio::spawn(async move {
            drain_capped(
                Some(crate::pty::PtyOutput(master)),
                output_limit,
                pipe_sender,
                PipeStream::Stdout,
                frame_limit,
                terminal_execution,
                reached,
                false,
            )
            .await
        });
        (merged, tokio::spawn(async { (Vec::new(), false) }))
    } else {
        let stdout_sender = pipe_sender.clone();
        let stdout_execution = stdout_sender.as_ref().map(|_| Arc::clone(&execution));
        let stderr_execution = pipe_sender.as_ref().map(|_| Arc::clone(&execution));
        (
            tokio::spawn(async move {
                drain_capped(
                    stdout,
                    output_limit,
                    stdout_sender,
                    PipeStream::Stdout,
                    frame_limit,
                    stdout_execution,
                    None,
                    true,
                )
                .await
            }),
            tokio::spawn(async move {
                drain_capped(
                    stderr,
                    output_limit,
                    pipe_sender,
                    PipeStream::Stderr,
                    frame_limit,
                    stderr_execution,
                    None,
                    true,
                )
                .await
            }),
        )
    };
    let (
        status,
        timed_out,
        cpu_exhausted,
        cpu_measurement_failed,
        aperture_exhausted,
        output_exhausted,
    ) = wait_for_child(
        &mut child,
        &cgroup,
        &execution,
        timeout,
        cpu_budget_micros,
        aperture.as_ref(),
        &output_bound,
    )
    .await;
    let (stdout, stdout_truncated) = stdout_task.await.unwrap_or_else(|_| (Vec::new(), true));
    let (stderr, stderr_truncated) = stderr_task.await.unwrap_or_else(|_| (Vec::new(), true));
    // Quiesce the whole run tree before the terminal counter sample. The shared mapping remains
    // owned by `aperture`, so killing and reaping the relay cannot invalidate it and no relay can
    // increment a counter after the observation is built.
    let cgroup_reconciled = reconcile_cgroup(&cgroup).await;
    let aperture_exhausted = ceiling_reached(aperture_exhausted, aperture.as_ref());
    // Asked once more after the wait, for exactly the reason `ceiling_reached` is: the drain raises
    // this flag from its own task, and a child that crosses the bound on its **last** write and
    // then exits loses the `select!` to `child.wait()`, leaving the supervision tick no chance to
    // read it. The flag outlives that race — it is a shared page this process owns — so without
    // this re-read the run is reported byte for byte like one that finished on its own: `exited`,
    // code 0, no refusal, with the transcript silently stopped at exactly the declared bound. That
    // is the silent degradation invariant 3 forbids, and it is what
    // `vectors/driver/pty-session-output-bound-ends-the-session.json` says must not happen.
    let output_exhausted = output_exhausted || output_bound.load(Ordering::Acquire);
    let applied_aperture = aperture
        .as_ref()
        .map(crate::egress::InstalledAperture::applied);
    drop(aperture);
    let forced_cancellation = timed_out
        || cpu_exhausted
        || aperture_exhausted
        || output_exhausted
        || execution.cancellation_requested.load(Ordering::Acquire);
    let capsule_reconciled = capsule.is_none_or(|capsule| capsule.directory.close().is_ok());
    let resolution_reconciled = resolution.is_none_or(crate::egress::GeneratedResolution::close);
    let mut observation = execution.observation.lock();
    observation.stdout = stdout;
    observation.stderr = stderr;
    observation.stdout_truncated = stdout_truncated;
    observation.stderr_truncated = stderr_truncated;
    observation.output_complete = true;
    observation.resource.observed_at = Utc::now();
    record_aperture(&mut observation, applied_aperture, aperture_exhausted);
    record_pipe_backpressure(&mut observation, &execution);
    record_terminal_output_bound(&mut observation, output_exhausted);
    match status {
        _ if !cgroup_reconciled
            || !capsule_reconciled
            || !resolution_reconciled
            || cpu_measurement_failed =>
        {
            observation.resource.state = ExecState::Unknown;
            observation.resource.exit = None;
        }
        Ok(status) => {
            let delivered = *execution.delivered_signal.lock();
            let signal = status.signal().and_then(signal_from_number).or_else(|| {
                delivered.filter(|delivered| status.code() == Some(128 + delivered.number()))
            });
            observation.resource.exit = Some(ExecExit {
                code: signal
                    .is_none()
                    .then(|| status.code().and_then(|code| u8::try_from(code).ok()))
                    .flatten(),
                signal,
            });
            observation.resource.state =
                if forced_cancellation || signal.is_some_and(|signal| Some(signal) == delivered) {
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

fn record_pipe_backpressure(observation: &mut ExecObservation, execution: &Execution) {
    if execution.pipe_backpressure.load(Ordering::Acquire) {
        observation.resource.refusal = Some(substrate_wire::ExecRefusal {
            class: substrate_wire::ErrorClass::Exhausted,
            code: "session.output-backpressure".to_owned(),
            message: "The raw-pipe live output queue was not drained within its declared bound."
                .to_owned(),
        });
    }
}

/// Whether this run reached its declared aperture ceiling — asked once more after the wait.
///
/// The supervision tick is not the only way to find out, and it must not be the only place that
/// looks. The relay stops the bytes and closes both halves of the connection
/// (`crate::egress::relay_body`); a child that notices the EOF and exits before the next 1 ms tick
/// wins the `child.wait()` arm of the `select!` below, so `wait_for_child` returns with the flag
/// still clear. The counters outlive that race — they are a shared page this process owns and the
/// forwarder is still alive here — so the run would otherwise be reported byte for byte like one
/// that finished on its own: `exited`, code 0, no refusal, with `bytes` summing to exactly the
/// declared ceiling. That is the silent degradation invariant 3 forbids, and the field ADR 0014
/// added exists precisely so it cannot happen.
fn ceiling_reached(
    observed_on_a_tick: bool,
    aperture: Option<&crate::egress::InstalledAperture>,
) -> bool {
    observed_on_a_tick || aperture.is_some_and(crate::egress::InstalledAperture::ceiling_exceeded)
}

/// What the aperture did, and the bound it hit, on the run's own observation.
///
/// The bytes that crossed, counted where they crossed. An aperture with no traffic still reports
/// itself: "installed and unused" and "never installed" are different observations. The refusal is
/// the one bound that names itself — `state` is `Cancelled` for a ceiling exactly as it is for a
/// timeout and a CPU budget, and what this adds is *which* bound, so an operator does not have to
/// tell a ceiling from a client cancel by reading the byte counts (ADR 0014).
fn record_aperture(
    observation: &mut ExecObservation,
    applied: Option<substrate_wire::AppliedAperture>,
    exhausted: bool,
) {
    if let Some(applied) = applied
        && let Some(confinement) = observation.resource.applied.as_mut()
    {
        confinement.network = AppliedNetwork::Aperture(applied);
    }
    if exhausted {
        observation.resource.refusal = Some(substrate_wire::ExecRefusal {
            class: substrate_wire::ErrorClass::Exhausted,
            code: "exec.aperture-byte-limit".to_owned(),
            message: "The declared egress aperture byte ceiling was reached.".to_owned(),
        });
    }
}

/// The bound a terminal session ends at, named on the run's own observation (ADR 0014's field).
///
/// `state` is already `Cancelled` here, exactly as for a timeout; what this adds is *which* bound.
/// The `exit` frame the attachment receives carries `state` and `exit` and no refusal — design 05
/// gave it no field for one — so this is read from the durable exec the client fetches afterwards
/// (`GET /v1/execs/{exec_id}`), which is the only place an output ceiling can be told from a client
/// cancel without counting bytes. An aperture ceiling keeps its own name where both could apply: it
/// is the bound that stopped the bytes first.
fn record_terminal_output_bound(observation: &mut ExecObservation, exhausted: bool) {
    if exhausted && observation.resource.refusal.is_none() {
        observation.resource.refusal = Some(substrate_wire::ExecRefusal {
            class: substrate_wire::ErrorClass::Exhausted,
            code: substrate_wire::SESSION_OUTPUT_LIMIT.to_owned(),
            message: "The declared output bound ended the terminal session.".to_owned(),
        });
    }
}

async fn wait_for_child(
    child: &mut Child,
    cgroup: &Cgroup,
    execution: &Execution,
    timeout: Duration,
    cpu_budget_micros: u64,
    aperture: Option<&crate::egress::InstalledAperture>,
    output_bound: &AtomicBool,
) -> (io::Result<ExitStatus>, bool, bool, bool, bool, bool) {
    let mut timed_out = false;
    let mut cpu_exhausted = false;
    let mut cpu_measurement_failed = false;
    let mut aperture_exhausted = false;
    let mut output_exhausted = false;
    let timeout_sleep = tokio::time::sleep(timeout);
    tokio::pin!(timeout_sleep);
    let mut cpu_poll = tokio::time::interval(Duration::from_millis(1));
    cpu_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let status = loop {
        tokio::select! {
            result = child.wait() => break result,
            () = &mut timeout_sleep => {
                timed_out = true;
                let _ = cgroup.kill_all();
                close_live_output(execution).await;
                break child.wait().await;
            }
            _ = cpu_poll.tick() => {
                if execution.pipe_backpressure.load(Ordering::Acquire) {
                    let _ = cgroup.kill_all();
                    close_live_output(execution).await;
                    break child.wait().await;
                }
                // The ceiling before the CPU budget, on the same tick: a run that spends its
                // budget waiting on an aperture it has already exhausted hit the ceiling, and the
                // refusal has to say so (ADR 0014).
                if aperture.is_some_and(crate::egress::InstalledAperture::ceiling_exceeded) {
                    aperture_exhausted = true;
                    let _ = cgroup.kill_all();
                    close_live_output(execution).await;
                    break child.wait().await;
                }
                // Reaching the output bound *ends* a pty session; it does not truncate it. Design
                // 05 gave the pty no `truncated` frame to deliver the statement on, and a terminal
                // stream has no per-stream offset for a client to rejoin at, so a transcript that
                // silently stopped would be unrecoverable. The client declared this bound.
                if output_bound.load(Ordering::Acquire) {
                    output_exhausted = true;
                    let _ = cgroup.kill_all();
                    close_live_output(execution).await;
                    break child.wait().await;
                }
                match cgroup.cpu_usage_micros() {
                    Ok(usage) if usage >= cpu_budget_micros => {
                        cpu_exhausted = true;
                        let _ = cgroup.kill_all();
                        close_live_output(execution).await;
                        break child.wait().await;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        cpu_measurement_failed = true;
                        let _ = cgroup.kill_all();
                        close_live_output(execution).await;
                        break child.wait().await;
                    }
                }
            }
        }
    };
    (
        status,
        timed_out,
        cpu_exhausted,
        cpu_measurement_failed,
        aperture_exhausted,
        output_exhausted,
    )
}

async fn close_live_output(execution: &Execution) {
    if let Some(pipe) = &execution.pipe {
        pipe.output.lock().await.close();
    }
}

struct ActivePermit {
    active: Arc<AtomicUsize>,
}

struct TrackingReservation {
    executions: Arc<Mutex<HashMap<String, Arc<Execution>>>>,
    reservations: Arc<Mutex<HashSet<String>>>,
    id: String,
    installed: bool,
}

impl TrackingReservation {
    fn acquire(
        executions: Arc<Mutex<HashMap<String, Arc<Execution>>>>,
        reservations: Arc<Mutex<HashSet<String>>>,
        id: &str,
        maximum: usize,
    ) -> Result<Self, DriverError> {
        let executions_guard = executions.lock();
        let mut reservations_guard = reservations.lock();
        if executions_guard
            .len()
            .saturating_add(reservations_guard.len())
            >= maximum
        {
            return Err(DriverError::exhausted(
                "exec.tracking-capacity",
                "The bounded exec observation capacity is exhausted.",
                "exec",
            ));
        }
        if executions_guard.contains_key(id) || !reservations_guard.insert(id.to_owned()) {
            return Err(DriverError {
                class: crate::DriverErrorClass::Conflict,
                code: "exec.already-exists",
                message: "Exec identity already exists.".to_owned(),
                address: Some("exec".to_owned()),
                retriable: false,
            });
        }
        drop(reservations_guard);
        drop(executions_guard);
        Ok(Self {
            executions,
            reservations,
            id: id.to_owned(),
            installed: false,
        })
    }

    fn install(mut self, execution: Arc<Execution>) {
        self.executions.lock().insert(self.id.clone(), execution);
        self.reservations.lock().remove(&self.id);
        self.installed = true;
    }
}

impl Drop for TrackingReservation {
    fn drop(&mut self) {
        if !self.installed {
            self.reservations.lock().remove(&self.id);
        }
    }
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

async fn contain_spawned(
    child: Child,
    cgroup: Cgroup,
    error: DriverError,
) -> DispatchOutcome<ExecObservation> {
    contain_spawned_with_reconciliation(child, cgroup, error, |cgroup| async move {
        reconcile_cgroup(&cgroup).await
    })
    .await
}

async fn contain_spawned_with_reconciliation<F, Fut>(
    mut child: Child,
    cgroup: Cgroup,
    error: DriverError,
    reconcile: F,
) -> DispatchOutcome<ExecObservation>
where
    F: FnOnce(Cgroup) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let _ = cgroup.kill_all();
    let _ = child.kill().await;
    let reaped = child.wait().await.is_ok();
    let absent = reconcile(cgroup).await;
    if reaped && absent {
        DispatchOutcome::ContainedAbsent(error)
    } else {
        DispatchOutcome::OutcomeUnknown(error)
    }
}

/// Three clones of the slave, one for each of the child's standard descriptors.
///
/// The master is not among them and never is: it is close-on-exec from allocation, so no fork of
/// this process can carry the terminal's other end into a sandbox.
fn child_terminal_descriptors(
    pair: &crate::pty::PtyPair,
) -> Result<Box<[OwnedFd; 3]>, DriverError> {
    let clone = || {
        pair.slave.try_clone().map_err(|error| {
            DriverError::failed(
                substrate_wire::SESSION_PTY_FAILED,
                format!("pty slave descriptor: {error}"),
            )
        })
    };
    Ok(Box::new([clone()?, clone()?, clone()?]))
}

fn contain_cgroup(cgroup: &Cgroup, error: DriverError) -> DispatchOutcome<ExecObservation> {
    if cgroup.prove_empty_and_remove().is_ok() {
        DispatchOutcome::ContainedAbsent(error)
    } else {
        DispatchOutcome::OutcomeUnknown(error)
    }
}

/// Drains one reader under the admitted output bound, live and durably.
///
/// `mark_truncation` is the raw-pipe truncation statement, and a terminal does not get one. Design
/// 13 removed truncation from the pty vocabulary on purpose — reaching the bound *ends* a terminal
/// session and names itself on the exec observation's refusal field — so writing
/// `[substrate: output truncated]` into a merged terminal transcript would both speak a statement
/// the published frame vocabulary has no branch for and destroy the last
/// `TRUNCATION_MARKER.len()` bytes the child actually wrote inside the bound.
async fn drain_capped<R>(
    reader: Option<R>,
    limit: usize,
    sender: Option<mpsc::Sender<PipeFrame>>,
    stream: PipeStream,
    frame_limit: usize,
    execution: Option<Arc<Execution>>,
    bound_reached: Option<Arc<AtomicBool>>,
    mark_truncation: bool,
) -> (Vec<u8>, bool)
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
            if let Some(reached) = bound_reached.as_ref() {
                reached.store(true, Ordering::Release);
            }
        }
        if let Some(sender) = &sender
            && execution
                .as_ref()
                .is_none_or(|execution| !execution.pipe_backpressure.load(Ordering::Acquire))
        {
            // The live channel carries only bytes retained under the same admitted output bound.
            // Continue draining excess child output without forwarding it so the child cannot
            // block and a consumer cannot observe more bytes than Substrate attested.
            for chunk in buffer[..retained].chunks(frame_limit) {
                match sender.try_send(PipeFrame {
                    stream,
                    bytes: chunk.to_vec(),
                }) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        if let Some(execution) = &execution {
                            execution.pipe_backpressure.store(true, Ordering::Release);
                            execution
                                .cancellation_requested
                                .store(true, Ordering::Release);
                        }
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
        }
    }
    if truncated {
        if !mark_truncation {
            stored.truncate(limit);
        } else if limit >= TRUNCATION_MARKER.len() {
            stored.truncate(limit - TRUNCATION_MARKER.len());
            stored.extend_from_slice(TRUNCATION_MARKER);
        } else {
            stored.truncate(limit);
        }
    }
    (stored, truncated)
}

async fn wait_terminal(execution: &Execution, limit: Duration) -> Result<(), DriverError> {
    wait_terminal_with_hook(execution, limit, || {}).await
}

async fn wait_terminal_with_hook<F>(
    execution: &Execution,
    limit: Duration,
    after_state_check: F,
) -> Result<(), DriverError>
where
    F: FnOnce(),
{
    let deadline = tokio::time::Instant::now() + limit;
    let mut after_state_check = Some(after_state_check);
    loop {
        let notified = execution.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if is_terminal(execution.observation.lock().resource.state) {
            return Ok(());
        }
        if let Some(hook) = after_state_check.take() {
            hook();
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
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired | ExecState::Unknown
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
    #[allow(clippy::result_large_err)] // The error preserves the typed dispatch posture.
    fn create(
        root: &Path,
        id: &str,
        input: &ExecStartInput,
    ) -> Result<Self, DispatchOutcome<ExecObservation>> {
        let name = format!("substrate-{id}");
        let path = root.join(&name);
        std::fs::create_dir(&path).map_err(|error| {
            DispatchOutcome::NotDispatched(DriverError::failed(
                "exec.cgroup-create-failed",
                format!("cgroup create: {error}"),
            ))
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
        if let Err(error) = result {
            return Err(contain_cgroup(&Self { path, name }, error));
        }
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

    /// This cgroup's `cgroup.procs`, which is how a forked process joins it by writing `0`.
    fn procs(&self) -> PathBuf {
        self.path.join("cgroup.procs")
    }

    fn kill_all(&self) -> Result<(), DriverError> {
        write_control(&self.path, "cgroup.kill", "1")
    }

    fn cpu_usage_micros(&self) -> Result<u64, DriverError> {
        let stats = std::fs::read_to_string(self.path.join("cpu.stat")).map_err(|error| {
            DriverError::failed(
                "exec.cgroup-read-failed",
                format!("cgroup cpu.stat: {error}"),
            )
        })?;
        stats
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                (fields.next() == Some("usage_usec"))
                    .then(|| fields.next()?.parse::<u64>().ok())
                    .flatten()
            })
            .ok_or_else(|| {
                DriverError::failed(
                    "exec.cgroup-read-failed",
                    "cgroup cpu.stat omitted usage_usec",
                )
            })
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
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    use base64::Engine as _;
    use sha2::{Digest as _, Sha256};
    use substrate_wire::{
        Base64Content, Base64Encoding, CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts,
        ExecEnvironment, ExecLimits, ExecStartInput, ExecState, ExecutionCapsuleFile,
        ExecutionCapsuleFileRole, ExecutionCapsuleInput, HostDriverKind, NamespaceFacts,
        NetworkMode, SandboxProfile, canonical_execution_capsule_hash,
    };

    use super::{
        Cgroup, ExecObservation, Execution, PipeState, PipeStream, ProcessRuntime,
        close_live_output, contain_spawned_with_reconciliation, drain_capped, is_secretish_name,
        is_terminal, wait_terminal_with_hook,
    };
    use crate::{DispatchOutcome, HostConfig};

    fn running_observation(id: &str) -> ExecObservation {
        ExecObservation {
            resource: substrate_wire::Exec {
                id: id.to_owned(),
                kind: substrate_wire::ExecKind::Exec,
                workspace: "ws_test".to_owned(),
                state: substrate_wire::ExecState::Running,
                observed_at: chrono::Utc::now(),
                requested: substrate_wire::ConfinementRequest {
                    capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                    network: NetworkMode::None,
                    aperture: None,
                    profile: SandboxProfile::Workspace,
                    required: true,
                },
                applied: None,
                exit: None,
                lease: None,
                refusal: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: false,
            cgroup: None,
            leader_pid: None,
        }
    }

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

    #[test]
    fn capsule_materialization_verifies_bytes_and_cleans_private_directory() {
        let root = tempfile::tempdir().expect("root");
        let mut config = HostConfig::minimum(root.path().join("workspaces"));
        let cgroup_root = root.path().join("cgroups");
        std::fs::create_dir(&cgroup_root).expect("cgroup root");
        config.cgroup_root = Some(cgroup_root);
        std::fs::create_dir_all(&config.capsule_root).expect("capsule root");
        let capability = CapabilitySnapshot {
            snapshot: format!("sha256:{}", "7".repeat(64)),
            driver: HostDriverKind::Host,
            driver_version: "test".to_owned(),
            config_generation: 1,
            probed_at: chrono::Utc::now(),
            valid_until: None,
            facts: CapabilityFacts::default(),
        };
        let runtime = ProcessRuntime::new(config.clone(), capability).expect("runtime");
        let bytes = b"#!/bin/sh\nprintf capsule";
        let file = ExecutionCapsuleFile {
            path: "bin/harness".to_owned(),
            role: ExecutionCapsuleFileRole::Runtime,
            executable: true,
            sha256: hex::encode(Sha256::digest(bytes)),
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        };
        let manifest_sha256 =
            canonical_execution_capsule_hash("bin/harness", std::slice::from_ref(&file))
                .expect("manifest");
        let mut capsule = ExecutionCapsuleInput {
            manifest_sha256,
            entrypoint: "bin/harness".to_owned(),
            files: vec![file],
        };
        let prepared = runtime
            .prepare_capsule(Some(&capsule))
            .expect("materializes")
            .expect("present");
        assert_eq!(
            std::fs::read(prepared.directory.path().join("bin/harness")).expect("read"),
            bytes
        );
        assert_eq!(prepared.applied.mount, "/runtime");
        prepared.directory.close().expect("cleanup");
        assert_eq!(
            std::fs::read_dir(&config.capsule_root)
                .expect("list capsule root")
                .count(),
            0
        );

        capsule.files[0].content.data = "dGFtcGVyZWQ=".to_owned();
        let error = runtime
            .prepare_capsule(Some(&capsule))
            .err()
            .expect("tamper refuses");
        assert_eq!(error.code, "capsule.invalid");
        assert_eq!(
            std::fs::read_dir(&config.capsule_root)
                .expect("list capsule root")
                .count(),
            0
        );
    }

    #[test]
    fn startup_reconciles_only_private_stale_capsule_directories() {
        let root = tempfile::tempdir().expect("root");
        let mut config = HostConfig::minimum(root.path().join("workspaces"));
        let cgroup_root = root.path().join("cgroups");
        std::fs::create_dir(&cgroup_root).expect("cgroup root");
        config.cgroup_root = Some(cgroup_root);
        std::fs::create_dir_all(config.capsule_root.join("capsule-crashed"))
            .expect("stale capsule");
        std::fs::write(
            config.capsule_root.join("capsule-crashed/bin"),
            b"stale runtime",
        )
        .expect("stale bytes");
        std::fs::create_dir_all(config.aperture_root().join("aperture-crashed"))
            .expect("stale aperture");
        std::fs::write(
            config.aperture_root().join("aperture-crashed/hosts"),
            b"stale generated mapping",
        )
        .expect("stale generated bytes");
        let capability = CapabilitySnapshot {
            snapshot: format!("sha256:{}", "7".repeat(64)),
            driver: HostDriverKind::Host,
            driver_version: "test".to_owned(),
            config_generation: 1,
            probed_at: chrono::Utc::now(),
            valid_until: None,
            facts: CapabilityFacts::default(),
        };
        ProcessRuntime::new(config.clone(), capability.clone()).expect("reconciles stale capsule");
        assert_eq!(
            std::fs::read_dir(&config.capsule_root)
                .expect("list capsule root")
                .count(),
            0
        );
        assert_eq!(
            std::fs::read_dir(config.aperture_root())
                .expect("list aperture root")
                .count(),
            0
        );

        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).expect("outside");
        std::fs::write(outside.join("keep"), b"operator data").expect("outside marker");
        symlink(&outside, config.capsule_root.join("capsule-symlink"))
            .expect("malicious stale link");
        let error = ProcessRuntime::new(config.clone(), capability.clone())
            .err()
            .expect("symlink refuses");
        assert_eq!(error.code, "capsule.reconcile-invalid");
        assert_eq!(
            std::fs::read(outside.join("keep")).expect("outside retained"),
            b"operator data"
        );

        std::fs::remove_file(config.capsule_root.join("capsule-symlink"))
            .expect("remove test symlink");
        symlink(&outside, config.aperture_root().join("aperture-symlink"))
            .expect("malicious stale aperture link");
        let error = ProcessRuntime::new(config.clone(), capability.clone())
            .err()
            .expect("aperture symlink refuses");
        assert_eq!(error.code, "aperture.reconcile-invalid");
        assert_eq!(
            std::fs::read(outside.join("keep")).expect("outside retained"),
            b"operator data"
        );
        std::fs::remove_file(config.aperture_root().join("aperture-symlink"))
            .expect("remove test aperture symlink");
        std::fs::create_dir(config.capsule_root.join("capsule-unproven"))
            .expect("unproven capsule");
        config.cgroup_root = None;
        let error = ProcessRuntime::new(
            config,
            CapabilitySnapshot {
                snapshot: format!("sha256:{}", "7".repeat(64)),
                driver: HostDriverKind::Host,
                driver_version: "test".to_owned(),
                config_generation: 1,
                probed_at: chrono::Utc::now(),
                valid_until: None,
                facts: CapabilityFacts::default(),
            },
        )
        .err()
        .expect("unproven tree state refuses stale cleanup");
        assert_eq!(error.code, "capsule.reconcile-unproven");
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
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            workspace: "ws_test".to_owned(),
            argv: vec!["/usr/bin/true".to_owned()],
            env: ExecEnvironment {
                allow: vec![],
                set: BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "8".repeat(64)),
                network: NetworkMode::None,
                aperture: None,
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
            capsule: None,
            lease_ttl_ms: None,
        };
        let DispatchOutcome::NotDispatched(error) = runtime
            .start("ex_test", std::path::Path::new("/does/not/exist"), &input)
            .await
        else {
            panic!("stale snapshot must refuse before dispatch");
        };
        assert_eq!(error.code, "exec.capability-stale");
    }

    #[tokio::test]
    async fn raw_pipe_refuses_when_hard_confinement_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("ws_test");
        std::fs::create_dir(&workspace).unwrap();
        let snapshot = format!("sha256:{}", "7".repeat(64));
        let capability = CapabilitySnapshot {
            snapshot: snapshot.clone(),
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
        let mut config = HostConfig::minimum(root.path());
        config.bubblewrap = std::env::current_exe().expect("test executable");
        let runtime = ProcessRuntime::new(config, capability).unwrap();
        let input = ExecStartInput {
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            workspace: "ws_test".to_owned(),
            argv: vec!["/usr/bin/true".to_owned()],
            env: ExecEnvironment {
                allow: vec![],
                set: BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: snapshot,
                network: NetworkMode::None,
                aperture: None,
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
            capsule: None,
            lease_ttl_ms: None,
        };
        let DispatchOutcome::NotDispatched(error) = runtime
            .start_pipe(
                "ex_pipe",
                &workspace,
                &substrate_wire::PipeSessionStartInput {
                    exec: input,
                    input_limit_bytes: 65_536,
                    frame_limit_bytes: 4_096,
                    queued_frames: 4,
                    mode: substrate_wire::SessionMode::Pipes,
                    window: None,
                },
            )
            .await
        else {
            panic!("raw pipes must not fall back to an unconfined process");
        };
        assert_eq!(error.code, "exec.sandbox-unavailable");
    }

    /// A terminal is never quietly downgraded to pipes, in either direction.
    ///
    /// Two refusals, and the ordering is the point of the first: the request with no published
    /// `sessions.pty` fact also carries **no window**, so it earns `session.window-invalid` too and
    /// the case only passes if the fact really is checked before the window shape — the order
    /// `0.9.0` states in `vectors/http/pty-session-unserved-outranks-a-missing-window.json`. The
    /// second request has the fact and a window and has lost the confinement floor, so it earns
    /// `exec.sandbox-unavailable`. Neither answer is a pipe session (design 13, invariant 3).
    #[tokio::test]
    async fn pty_session_refused_without_confinement() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("ws_test");
        std::fs::create_dir(&workspace).unwrap();
        let snapshot = format!("sha256:{}", "7".repeat(64));
        let facts = |pty: Option<bool>| CapabilityFacts {
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
            sessions_pty: pty,
            ..CapabilityFacts::default()
        };
        for (published, window, expected) in [
            (None, None, substrate_wire::SESSION_PTY_UNSERVED),
            (
                Some(true),
                Some(substrate_wire::PtyWindow {
                    columns: 80,
                    rows: 24,
                }),
                "exec.sandbox-unavailable",
            ),
        ] {
            let capability = CapabilitySnapshot {
                snapshot: snapshot.clone(),
                driver: HostDriverKind::Host,
                driver_version: "test".to_owned(),
                config_generation: 7,
                probed_at: chrono::Utc::now(),
                valid_until: None,
                facts: facts(published),
            };
            let mut config = HostConfig::minimum(root.path());
            config.bubblewrap = std::env::current_exe().expect("test executable");
            let runtime = ProcessRuntime::new(config, capability).unwrap();
            let DispatchOutcome::NotDispatched(error) = runtime
                .start_pipe(
                    "ex_pty",
                    &workspace,
                    &substrate_wire::PipeSessionStartInput {
                        exec: pty_exec_input(&snapshot),
                        input_limit_bytes: 65_536,
                        frame_limit_bytes: 4_096,
                        queued_frames: 4,
                        mode: substrate_wire::SessionMode::Pty,
                        window,
                    },
                )
                .await
            else {
                panic!("a terminal must never be served as a pipe session instead");
            };
            assert_eq!(error.code, expected, "published fact: {published:?}");
        }
    }

    /// Losing the attachment kills the whole tree, not the process the client was talking to.
    ///
    /// This is `terminate_pipe_session`'s driver call — `KILL` with no grace — against a pty
    /// session whose shell has forked a child of its own. What must be true afterwards: a terminal
    /// observation of `cancelled`, an empty cgroup, and a master that reads end of file rather than
    /// a transcript that simply stops.
    ///
    /// Delegated only. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
    /// process is inside, the case is **absent, never reported as passed** (invariant 3).
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines)] // One delegated session lifecycle, start to terminal.
    async fn pty_session_kills_tree_on_attachment_loss() {
        let Some(delegated) = delegated_cgroup_root("attachment-loss") else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("ws_test");
        std::fs::create_dir(&workspace).unwrap();
        let snapshot = format!("sha256:{}", "7".repeat(64));
        let mut config = HostConfig::minimum(root.path());
        config.cgroup_root = Some(delegated.clone());
        let capability = CapabilitySnapshot {
            snapshot: snapshot.clone(),
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
                sessions_pty: Some(true),
                ..CapabilityFacts::default()
            },
        };
        let runtime = ProcessRuntime::new(config, capability).expect("runtime");
        let mut input = pty_exec_input(&snapshot);
        input.argv = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "sleep 300 & printf 'ready\\n'; sleep 300".to_owned(),
        ];
        input.limits.timeout_ms = 60_000;
        let observed = match runtime
            .start_pipe(
                "ex_ptykill",
                &workspace,
                &substrate_wire::PipeSessionStartInput {
                    exec: input,
                    input_limit_bytes: 65_536,
                    frame_limit_bytes: 4_096,
                    queued_frames: 4,
                    mode: substrate_wire::SessionMode::Pty,
                    window: Some(substrate_wire::PtyWindow {
                        columns: 80,
                        rows: 24,
                    }),
                },
            )
            .await
        {
            DispatchOutcome::Observed(observed) => observed,
            DispatchOutcome::NotDispatched(error)
            | DispatchOutcome::ContainedAbsent(error)
            | DispatchOutcome::OutcomeUnknown(error) => {
                panic!(
                    "the delegated lane must dispatch a pty session: {} {}",
                    error.code, error.message
                )
            }
        };
        let cgroup = delegated.join(observed.cgroup.clone().expect("a pty session has a cgroup"));
        let mut transcript = Vec::new();
        while !transcript.windows(5).any(|window| window == b"ready") {
            let frame = runtime
                .read_pipe("ex_ptykill", Duration::from_secs(10))
                .await
                .expect("the terminal reads")
                .expect("the shell announced itself");
            transcript.extend_from_slice(&frame.bytes);
        }

        let terminal = runtime
            .signal(
                "ex_ptykill",
                &substrate_wire::ExecSignalInput {
                    signal: substrate_wire::Signal::Kill,
                    grace_ms: 0,
                },
            )
            .await
            .expect("losing the attachment terminates the session");
        assert_eq!(terminal.resource.state, ExecState::Cancelled);
        assert_eq!(
            std::fs::read_to_string(cgroup.join("cgroup.procs"))
                .unwrap_or_default()
                .trim(),
            "",
            "the whole tree is gone, not just the shell the client was talking to"
        );
        assert!(
            runtime
                .read_pipe("ex_ptykill", Duration::from_secs(5))
                .await
                .expect("the terminal reads")
                .is_none(),
            "a killed terminal reads end of file, it does not simply stop"
        );
    }

    /// A private, controller-carrying child of the delegated cgroup v2 subtree, or nothing.
    ///
    /// The daemon's own lane hands `--cgroup-root` the delegation root and lets the startup probe
    /// enable the controllers; this test constructs the runtime directly, so it does the same two
    /// writes itself. Absent when the variable is unset — the case then makes no claim at all,
    /// rather than reporting a green it did not earn (invariant 3).
    fn delegated_cgroup_root(name: &str) -> Option<std::path::PathBuf> {
        let root = std::path::PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
        let path = root.join(format!("host-pty-{name}"));
        if path.is_dir() {
            let _replaced = std::fs::remove_dir_all(&path);
        }
        std::fs::create_dir(&path).ok()?;
        let controllers = "+cpu +memory +pids";
        std::fs::write(root.join("cgroup.subtree_control"), controllers).ok()?;
        std::fs::write(path.join("cgroup.subtree_control"), controllers).ok()?;
        Some(path)
    }

    fn pty_exec_input(snapshot: &str) -> ExecStartInput {
        ExecStartInput {
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            workspace: "ws_test".to_owned(),
            argv: vec!["/bin/sh".to_owned()],
            env: ExecEnvironment {
                allow: vec![],
                set: BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: snapshot.to_owned(),
                network: NetworkMode::None,
                aperture: None,
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
            capsule: None,
            lease_ttl_ms: Some(60_000),
        }
    }

    #[tokio::test]
    async fn live_drain_preserves_stream_and_bounded_capture() {
        use tokio::io::AsyncWriteExt as _;

        let (mut writer, reader) = tokio::io::duplex(1024);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let drain = tokio::spawn(drain_capped(
            Some(reader),
            4,
            Some(sender),
            PipeStream::Stdout,
            64,
            None,
            true,
        ));
        writer.write_all(b"abcdef").await.unwrap();
        drop(writer);
        let frame = receiver.recv().await.unwrap();
        assert_eq!(frame.stream, PipeStream::Stdout);
        assert_eq!(frame.bytes, b"abcd");
        assert!(receiver.recv().await.is_none());
        let (captured, truncated) = drain.await.unwrap();
        assert_eq!(captured, b"abcd");
        assert!(truncated);
    }

    #[tokio::test]
    async fn saturating_a_live_queue_is_terminal_without_falsifying_durable_truncation() {
        use tokio::io::AsyncWriteExt as _;

        let (mut writer, reader) = tokio::io::duplex(1_024);
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let execution = std::sync::Arc::new(Execution::new(
            running_observation("ex_saturated_pipe"),
            Some(PipeState {
                stdin: tokio::sync::Mutex::new(None),
                output: tokio::sync::Mutex::new(receiver),
                input_bytes: AtomicU64::new(0),
                input_limit: 1_024,
                frame_limit: 1,
                terminal: None,
            }),
        ));
        let drain = tokio::spawn(drain_capped(
            Some(reader),
            64,
            Some(sender),
            PipeStream::Stdout,
            1,
            Some(std::sync::Arc::clone(&execution)),
            None,
            true,
        ));
        writer
            .write_all(b"queue saturation must not block timeout")
            .await
            .unwrap();
        drop(writer);
        tokio::task::yield_now().await;
        close_live_output(&execution).await;
        let (captured, truncated) = tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("closed receiver releases drain")
            .unwrap();
        assert!(!captured.is_empty());
        assert!(!truncated, "all bytes still fit the durable capture bound");
        assert!(
            execution
                .pipe_backpressure
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            execution
                .cancellation_requested
                .load(std::sync::atomic::Ordering::Acquire)
        );
    }

    #[test]
    fn cumulative_cpu_usage_is_read_from_the_exec_cgroup() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("cpu.stat"),
            "usage_usec 1234\nuser_usec 1000\nsystem_usec 234\n",
        )
        .unwrap();
        let cgroup = Cgroup {
            path: directory.path().to_path_buf(),
            name: "substrate-ex_cpu".to_owned(),
        };
        assert_eq!(cgroup.cpu_usage_micros().unwrap(), 1_234);
    }

    #[test]
    fn stale_running_acknowledgement_cannot_discard_newer_terminal_observation() {
        let capability = CapabilitySnapshot {
            snapshot: format!("sha256:{}", "7".repeat(64)),
            driver: HostDriverKind::Host,
            driver_version: "test".to_owned(),
            config_generation: 1,
            probed_at: chrono::Utc::now(),
            valid_until: None,
            facts: CapabilityFacts::default(),
        };
        let runtime = ProcessRuntime::new(HostConfig::minimum("/does/not/exist"), capability)
            .expect("runtime");
        let mut running = running_observation("ex_ack_race");
        let execution = std::sync::Arc::new(Execution::new(running.clone(), None));
        runtime.executions.lock().insert(
            running.resource.id.clone(),
            std::sync::Arc::clone(&execution),
        );

        let stale_running = running.clone();
        running.resource.state = substrate_wire::ExecState::Exited;
        running.resource.exit = Some(substrate_wire::ExecExit {
            code: Some(0),
            signal: None,
        });
        running.output_complete = true;
        *execution.observation.lock() = running.clone();

        runtime.acknowledge(&stale_running);
        assert_eq!(
            runtime.observe("ex_ack_race").expect("terminal retained"),
            running
        );
        runtime.acknowledge(&running);
        assert!(runtime.observe("ex_ack_race").is_err());
    }

    #[test]
    fn expired_exec_is_terminal_for_exact_acknowledgement() {
        assert!(is_terminal(ExecState::Expired));
    }

    #[tokio::test]
    async fn terminal_notify_between_state_check_and_wait_is_not_lost() {
        let mut running = running_observation("ex_notify_race");
        running.resource.exit = None;
        running.output_complete = false;
        let execution = std::sync::Arc::new(Execution::new(running, None));
        let hook_execution = std::sync::Arc::clone(&execution);
        wait_terminal_with_hook(
            &execution,
            std::time::Duration::from_millis(100),
            move || {
                hook_execution.observation.lock().resource.state = ExecState::Exited;
                hook_execution.notify.notify_waiters();
            },
        )
        .await
        .expect("registered waiter receives the transition");
    }

    #[tokio::test]
    async fn post_spawn_failure_with_proven_empty_cgroup_is_contained_absent() {
        let child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn contained child");
        let cgroup = Cgroup {
            path: std::path::PathBuf::from("/does/not/exist"),
            name: "substrate-ex_contained".to_owned(),
        };
        let error = crate::DriverError::failed(
            "exec.post-spawn-failed",
            "injected failure after process spawn",
        );

        let outcome =
            contain_spawned_with_reconciliation(child, cgroup, error, |_| async { true }).await;

        let DispatchOutcome::ContainedAbsent(error) = outcome else {
            panic!("reaped child plus proven-empty cgroup must be contained absent");
        };
        assert_eq!(error.code, "exec.post-spawn-failed");
    }

    #[tokio::test]
    async fn post_spawn_failure_with_failed_cgroup_cleanup_is_outcome_unknown() {
        let child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn uncontained child");
        let cgroup = Cgroup {
            path: std::path::PathBuf::from("/does/not/exist"),
            name: "substrate-ex_unknown".to_owned(),
        };
        let error = crate::DriverError::failed(
            "exec.post-spawn-failed",
            "injected failure after process spawn",
        );

        let outcome =
            contain_spawned_with_reconciliation(child, cgroup, error, |_| async { false }).await;

        let DispatchOutcome::OutcomeUnknown(error) = outcome else {
            panic!("unproven cgroup cleanup must remain outcome unknown");
        };
        assert_eq!(error.code, "exec.post-spawn-failed");
    }
}
