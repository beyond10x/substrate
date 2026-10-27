//! The `macos` adapter: a host process under a Seatbelt profile (ADR 0037, ADR 0109 S1).
//!
//! Each start renders a [`SeatbeltPolicy`] whose only writable location is the workspace, whose
//! read-only locations are the declared roots, and whose only executable is the program itself,
//! and runs the program through `/usr/bin/sandbox-exec` with the run's environment and nothing
//! else.
//!
//! It serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
//! `filesystem_view` and `audit`. It proves `paths_are_host_paths` (a declared root is visible at
//! its host path and nowhere else) and `ipc_signals_contained` (the profile allows `signal` only
//! to `(target same-sandbox)`). ADR 0037 "What Seatbelt cannot confine": no pids or memory bound,
//! no egress forwarder and no ingress, so `limits`, `network_egress` and `network_ingress` are not
//! served; a child can leave the process group with `setsid`, so `whole_tree_kill` is not proved.
//!
//! Since `story:macos-seatbelt-policy-runs-cargo-test` (ADR 0109 S2) it also proves the three
//! facts a Rust build needs, each only when [`seatbelt::probe_build`] saw Seatbelt enforce it at
//! open:
//!
//! - `sealed_paths`: a workspace-relative path stays unwritable, and so does each directory between
//!   the workspace and it. A sealed path reached through a symbolic link is refused
//!   `exec.sandbox-path-unrepresentable`, because the rule would name another path than the one
//!   the kernel checks.
//! - `exec_subpaths`: programs beneath a listed directory may run, and the directory is readable
//!   and mappable as code. A relative entry is beneath the workspace (`target`); an absolute one
//!   must lie in the workspace, a declared read-only root, or a system or developer directory
//!   ([`seatbelt::SYSTEM_EXEC_SUBPATHS`], [`seatbelt::developer_root`]), else it is refused
//!   `exec.sandbox-exec-subpath-undeclared`. A policy with an exec subpath may read
//!   [`seatbelt::XCODE_SELECT_LINK`].
//! - `private_tmpdir`: each start creates a directory beside the host's temporary directory,
//!   writable by that run alone, sets `TMPDIR` to it, and removes it before `wait` returns. A
//!   render cannot name a directory only a start creates, so it is `confinement.render-unserved`.
//!
//! `open` fails closed: off macOS, or when [`seatbelt::probe`] does not show the sandbox enforcing
//! a deny-default profile, it answers `confinement.adapter-unserved`. Nothing falls back to an
//! unconfined run.

use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use substrate_wire::{
    MAX_READ_ONLY_ROOTS, READ_ONLY_FILES_UNSERVED, READ_ONLY_ROOT_ABSENT, READ_ONLY_ROOT_INVALID,
    READ_ONLY_ROOT_NOT_A_DIRECTORY, ReadOnlyRoot, Signal,
};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::watch;

use super::{
    AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, RenderedCommand, RunExit, RunId, RunObservation,
    RunOutput, RunState,
};
use crate::DriverError;
use crate::seatbelt::{self, BuildProof, SeatbeltPolicy};

/// Each output stream is kept up to this many bytes and drained past it.
pub const MACOS_OUTPUT_LIMIT_BYTES: usize = 1_048_576;

/// Runs one adapter tracks; past it, ended runs are forgotten oldest first, and a start with every
/// tracked run still running is refused `exec.tracking-capacity`.
pub const MACOS_MAX_TRACKED_RUNS: usize = 128;

/// How long a run, its leader reaped and its output closed, waits for the rest of its process
/// group to be gone, killed members reaped by whoever inherited them, before it reports itself
/// ended anyway.
pub const MACOS_GROUP_SETTLE: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether any process, a zombie included, is still in process group `group`.
fn group_remains(group: i32) -> bool {
    // SAFETY: signal 0 checks existence and permission and delivers nothing; no memory is involved.
    let present = unsafe { libc::kill(-group, 0) } == 0;
    present || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The story that made this adapter prove [`S2_FACTS`].
pub const S2_STORY: &str = "story:macos-seatbelt-policy-runs-cargo-test";

/// The facts [`S2_STORY`] added to the `macos` declared set: each is proved when
/// [`seatbelt::probe_build`] shows it at open, and refused `confinement.fact-unserved` otherwise.
pub const S2_FACTS: [Fact; 3] = [Fact::SealedPaths, Fact::ExecSubpaths, Fact::PrivateTmpdir];

/// What opening `macos` takes. Nothing yet: the adapter proves what it serves on the host itself.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MacosRequest {}

pub(crate) fn open(
    _request: MacosRequest,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    if !cfg!(target_os = "macos") {
        return super::unserved(
            AdapterName::Macos,
            "the macos adapter needs macOS and its Seatbelt sandbox",
        );
    }
    seatbelt::probe().map_err(|error| ConfinementError::AdapterUnserved {
        adapter: AdapterName::Macos,
        detail: format!("the Seatbelt probe failed: {error}"),
    })?;
    Ok(Arc::new(MacosAdapter::with_facts(proved(
        seatbelt::probe_build(),
    ))))
}

/// `macos`'s declared set: what it proves when every build probe holds.
pub fn proof() -> AdapterFacts {
    proved(BuildProof {
        sealed_paths: true,
        exec_subpaths: true,
        private_tmpdir: true,
    })
}

/// What `macos` proves given what the build probes showed.
fn proved(build: BuildProof) -> AdapterFacts {
    AdapterFacts::new(
        AdapterName::Macos,
        None,
        &[
            (ContractFamily::ProcessRun, true),
            (ContractFamily::ProcessWatch, true),
            (ContractFamily::OutputRead, true),
            (ContractFamily::Wait, true),
            (ContractFamily::Workspace, true),
            (ContractFamily::FilesystemView, true),
            (ContractFamily::NetworkEgress, false),
            (ContractFamily::NetworkIngress, false),
            (ContractFamily::Limits, false),
            (ContractFamily::Audit, true),
        ],
        &[
            (Fact::PathsAreHostPaths, true),
            (Fact::IpcSignalsContained, true),
            (Fact::WholeTreeKill, false),
            (Fact::SealedPaths, build.sealed_paths),
            (Fact::ExecSubpaths, build.exec_subpaths),
            (Fact::PrivateTmpdir, build.private_tmpdir),
        ],
    )
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Running,
    Ended(RunExit),
    Unknown,
}

#[derive(Default)]
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

struct MacosRun {
    id: RunId,
    pid: i32,
    phase: watch::Receiver<Phase>,
    captured: Arc<Mutex<Captured>>,
    /// Set, under this lock, once the leader has exited and the run's output has closed, just
    /// before the leader is reaped: while it is false the leader is at least a zombie, so its pid
    /// still names this run's process group.
    reaped: Arc<Mutex<bool>>,
}

/// Blocks until `pid` has exited, without reaping it.
fn await_exit_unreaped(pid: i32) {
    let Ok(id) = libc::id_t::try_from(pid) else {
        return;
    };
    loop {
        // SAFETY: `siginfo_t` is plain data, and `waitid` writes into the one we own.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `WNOWAIT` leaves the child waitable by its owner.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                id,
                &raw mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

pub struct MacosAdapter {
    facts: AdapterFacts,
    runs: Mutex<Vec<Arc<MacosRun>>>,
    sequence: Mutex<u64>,
}

impl MacosAdapter {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_facts(proof())
    }

    fn with_facts(facts: AdapterFacts) -> Self {
        Self {
            facts,
            runs: Mutex::new(Vec::new()),
            sequence: Mutex::new(0),
        }
    }

    fn run(&self, id: &RunId) -> Result<Arc<MacosRun>, ConfinementError> {
        self.runs
            .lock()
            .iter()
            .find(|run| &run.id == id)
            .cloned()
            .ok_or_else(super::unknown_run)
    }

    fn observation(run: &MacosRun) -> RunObservation {
        let (state, exit) = match *run.phase.borrow() {
            Phase::Running => (RunState::Running, None),
            Phase::Ended(exit) => (RunState::Ended, Some(exit)),
            Phase::Unknown => (RunState::Unknown, None),
        };
        RunObservation {
            run: run.id.clone(),
            adapter: AdapterName::Macos,
            inner: None,
            declared_insecure: None,
            state,
            exit,
            memory_oom_kills: None,
        }
    }

    /// Makes room for one more run, forgetting ended runs oldest first.
    fn reserve(&self) -> Result<(), ConfinementError> {
        let mut runs = self.runs.lock();
        while runs.len() >= MACOS_MAX_TRACKED_RUNS {
            let Some(ended) = runs
                .iter()
                .position(|run| !matches!(*run.phase.borrow(), Phase::Running))
            else {
                return Err(DriverError::exhausted(
                    "exec.tracking-capacity",
                    "Every tracked macos run is still running.",
                    "run",
                )
                .into());
            };
            runs.remove(ended);
        }
        Ok(())
    }

    /// The admitted workspace, resolved; absent or not a directory is refused.
    fn workspace(admitted: &AdmittedProfile) -> Result<PathBuf, ConfinementError> {
        let workspace = super::workspace_of(admitted)?;
        if !workspace.is_dir() {
            return Err(DriverError::not_found().into());
        }
        canonical(workspace, "workspace")
    }

    /// The Seatbelt policy one start of `program` runs under: the workspace, the declared roots,
    /// the private `TMPDIR` when the start created one, the sealed paths and the exec subpaths,
    /// each after the refusals a start applies to it.
    fn policy(
        admitted: &AdmittedProfile,
        program: &Path,
        private_tmpdir: Option<&Path>,
    ) -> Result<SeatbeltPolicy, ConfinementError> {
        let workspace = Self::workspace(admitted)?;
        let profile = admitted.profile();
        let mut policy = SeatbeltPolicy::new(&workspace).executable(program);
        let mut declared = vec![workspace.clone()];
        for root in &profile.read_only_roots {
            let root = read_only_root(root, &workspace)?;
            declared.push(root.clone());
            policy = policy.read_only(root);
        }
        if let Some(directory) = private_tmpdir {
            policy = policy.writable(directory);
        }
        for path in &profile.sealed_paths {
            policy = policy.sealed(sealed_path(&workspace, path)?);
        }
        for subpath in &profile.exec_subpaths {
            policy = policy.exec_subpath(exec_subpath(&workspace, &declared, subpath)?);
        }
        Ok(policy)
    }

    /// The confined command one start would spawn: the Seatbelt launcher, its profile, the
    /// program and its arguments, in the workspace or the workdir beneath it, with `TMPDIR` set to
    /// the private directory when there is one. Nothing is spawned.
    fn command(
        admitted: &AdmittedProfile,
        command: &Command,
        private_tmpdir: Option<&Path>,
    ) -> Result<std::process::Command, ConfinementError> {
        super::check_command(command)?;
        crate::environment::check(&command.env)?;
        let program = Path::new(&command.argv[0]);
        if !program.is_absolute() {
            return Err(DriverError::refused(
                seatbelt::PATH_UNREPRESENTABLE,
                "The program must be an absolute path: a confined run has no search path.",
                "argv",
            )
            .into());
        }
        let policy = Self::policy(admitted, program, private_tmpdir)?;
        let workspace = super::workspace_of(admitted)?;
        let canonical_workspace = Self::workspace(admitted)?;
        let profile = admitted.profile();
        let directory = match &profile.workdir {
            None => canonical_workspace,
            Some(workdir) => {
                let relative = super::relative_workdir(workdir)?;
                let missing = || {
                    ConfinementError::from(DriverError::refused(
                        substrate_wire::EXEC_WORKDIR_MISSING,
                        "The working directory is not a directory beneath the workspace.",
                        "workdir",
                    ))
                };
                // Every component a directory itself, never a symbolic link, as `linux` and
                // `local` require (design 26).
                let mut directory = workspace.clone();
                for component in relative.components() {
                    directory.push(component);
                    let entry = std::fs::symlink_metadata(&directory).map_err(|_| missing())?;
                    if !entry.file_type().is_dir() {
                        return Err(missing());
                    }
                }
                let directory = std::fs::canonicalize(&directory).map_err(|_| missing())?;
                if !directory.is_dir() || !directory.starts_with(&canonical_workspace) {
                    return Err(missing());
                }
                directory
            }
        };
        let mut confined = policy.command(program, &command.argv[1..])?;
        confined.envs(&command.env);
        if let Some(private) = private_tmpdir {
            // The profile requires it, so it wins over a `TMPDIR` the command names.
            confined.env("TMPDIR", private);
        }
        confined
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(confined)
    }
}

fn canonical(path: &Path, address: &str) -> Result<PathBuf, ConfinementError> {
    std::fs::canonicalize(path).map_err(|error| {
        DriverError::refused(
            seatbelt::PATH_UNREPRESENTABLE,
            format!("cannot resolve {}: {error}", path.display()),
            address,
        )
        .into()
    })
}

/// A sealed path beneath the resolved workspace, refused when any part of it that exists is a
/// symbolic link: the rule would seal the link's target and leave the link itself replaceable.
fn sealed_path(workspace: &Path, path: &str) -> Result<PathBuf, ConfinementError> {
    seatbelt::relative_literal(path)?;
    let mut current = workspace.to_path_buf();
    for name in path.split('/') {
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(DriverError::refused(
                    seatbelt::PATH_UNREPRESENTABLE,
                    format!(
                        "the sealed path {path} is reached through the symbolic link {}",
                        current.display()
                    ),
                    "sealed_paths",
                )
                .into());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(workspace.join(path))
}

/// An exec subpath, resolved: a relative one beneath the workspace (it need not exist yet), an
/// absolute one only inside the workspace, a declared root, or a system or developer directory.
fn exec_subpath(
    workspace: &Path,
    declared: &[PathBuf],
    subpath: &str,
) -> Result<PathBuf, ConfinementError> {
    if !subpath.starts_with('/') {
        seatbelt::relative_literal(subpath)?;
        return Ok(workspace.join(subpath));
    }
    seatbelt::absolute_literal(subpath)?;
    let resolved = canonical(Path::new(subpath), "exec_subpaths")?;
    let system = seatbelt::SYSTEM_EXEC_SUBPATHS
        .iter()
        .filter_map(|directory| std::fs::canonicalize(directory).ok())
        .chain(seatbelt::developer_root());
    if declared
        .iter()
        .cloned()
        .chain(system)
        .any(|base| resolved.starts_with(base))
    {
        return Ok(resolved);
    }
    Err(DriverError::refused(
        seatbelt::EXEC_SUBPATH_UNDECLARED,
        format!(
            "{subpath} is outside the workspace, every declared root and the system and developer \
             directories"
        ),
        "exec_subpaths",
    )
    .into())
}

/// A fresh private `TMPDIR` for one start, beside the host's temporary directory, and its resolved
/// path. Dropping it removes it.
fn private_tmpdir() -> Result<(tempfile::TempDir, PathBuf), ConfinementError> {
    let failed = |error: std::io::Error| {
        ConfinementError::from(DriverError::failed(
            "exec.private-tmpdir-failed",
            format!("macos private TMPDIR: {error}"),
        ))
    };
    let directory = tempfile::Builder::new()
        .prefix("substrate-macos-tmp-")
        .tempdir()
        .map_err(failed)?;
    let resolved = std::fs::canonicalize(directory.path()).map_err(failed)?;
    Ok((directory, resolved))
}

/// Whether `path` is absolute and normal: no empty, `.` or `..` segment and no trailing slash.
fn is_absolute_normal(path: &str) -> bool {
    path == "/"
        || path.strip_prefix('/').is_some_and(|rest| {
            rest.split('/')
                .all(|segment| !matches!(segment, "" | "." | ".."))
        })
}

/// A declared root's host directory, resolved, after the refusals a start applies to it: absent,
/// not a directory (a symlink included), or overlapping the writable workspace, which would leave
/// the overlap writable.
fn read_only_root(root: &ReadOnlyRoot, workspace: &Path) -> Result<PathBuf, ConfinementError> {
    let host = Path::new(&root.host_path);
    let metadata = match std::fs::symlink_metadata(host) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DriverError::refused(
                READ_ONLY_ROOT_ABSENT,
                format!("{} is not present on this host", root.host_path),
                "read_only_roots",
            )
            .into());
        }
        Err(error) => {
            return Err(DriverError::refused(
                READ_ONLY_ROOT_ABSENT,
                format!("{} is not reachable: {error}", root.host_path),
                "read_only_roots",
            )
            .into());
        }
    };
    if !metadata.is_dir() {
        return Err(DriverError::refused(
            READ_ONLY_ROOT_NOT_A_DIRECTORY,
            format!("{} is not a directory", root.host_path),
            "read_only_roots",
        )
        .into());
    }
    let resolved = canonical(host, "read_only_roots")?;
    if resolved.starts_with(workspace) || workspace.starts_with(&resolved) {
        return Err(DriverError::refused(
            READ_ONLY_ROOT_INVALID,
            format!(
                "{} overlaps the writable workspace, so it cannot be read-only",
                root.host_path
            ),
            "read_only_roots",
        )
        .into());
    }
    Ok(resolved)
}

async fn drain(mut stream: impl AsyncRead + Unpin, captured: Arc<Mutex<Captured>>, stdout: bool) {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let mut captured = captured.lock();
        let captured = &mut *captured;
        let (kept, truncated) = if stdout {
            (&mut captured.stdout, &mut captured.stdout_truncated)
        } else {
            (&mut captured.stderr, &mut captured.stderr_truncated)
        };
        let room = MACOS_OUTPUT_LIMIT_BYTES.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
        *truncated |= read > room;
    }
}

impl MacosAdapter {
    /// Spawns on the calling task's Tokio runtime; `start` is its one caller.
    fn start_run(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        admitted.check_on(&self.facts, true)?;
        let private = if admitted.profile().private_tmpdir {
            Some(private_tmpdir()?)
        } else {
            None
        };
        let confined = Self::command(
            admitted,
            command,
            private.as_ref().map(|(_, path)| path.as_path()),
        )?;
        self.reserve()?;
        let mut child = tokio::process::Command::from(confined)
            .spawn()
            .map_err(|error| {
                DriverError::failed("exec.spawn-failed", format!("macos spawn: {error}"))
            })?;
        let pid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .ok_or_else(|| DriverError::failed("exec.spawn-failed", "macos spawn: no pid"))?;
        let id = {
            let mut sequence = self.sequence.lock();
            *sequence += 1;
            RunId::new(format!("macos-{pid}-{sequence}"))
        };
        let captured = Arc::new(Mutex::new(Captured::default()));
        let (phase, observed) = watch::channel(Phase::Running);
        let stdout = child
            .stdout
            .take()
            .map(|stream| tokio::spawn(drain(stream, Arc::clone(&captured), true)));
        let stderr = child
            .stderr
            .take()
            .map(|stream| tokio::spawn(drain(stream, Arc::clone(&captured), false)));
        let reaped = Arc::new(Mutex::new(false));
        let reaping = Arc::clone(&reaped);
        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || await_exit_unreaped(pid)).await;
            // A process the run started may still hold its output, and the run is not over until
            // it lets go. The leader stays a zombie meanwhile, pinning the group id for `signal`.
            for reader in [stdout, stderr].into_iter().flatten() {
                let _ = reader.await;
            }
            *reaping.lock() = true;
            let ended = child.wait().await;
            // A member killed while the run ran is a zombie until whoever inherited it reaps it;
            // the run is over once the group is empty. A member that outlives the bound is not
            // tracked (the group id may since name another process), and the run ends anyway.
            let settle = tokio::time::Instant::now() + MACOS_GROUP_SETTLE;
            while group_remains(pid) && tokio::time::Instant::now() < settle {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            // Removed before the run is observed ended, so `wait` never returns while it exists.
            drop(private);
            let _ = phase.send(match ended {
                Ok(status) => Phase::Ended(RunExit {
                    code: status.code(),
                    signal: status.signal(),
                }),
                Err(_) => Phase::Unknown,
            });
        });
        self.runs.lock().push(Arc::new(MacosRun {
            id: id.clone(),
            pid,
            phase: observed,
            captured,
            reaped,
        }));
        admitted.audit_start(&id, &command.argv[0]);
        Ok(id)
    }
}

#[async_trait]
impl ConfinementAdapter for MacosAdapter {
    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    /// What the Seatbelt policy cannot carry: a sealed secret slot, a writable root outside the
    /// workspace, a single read-only file, more roots than the wire bound, and a root that would
    /// appear anywhere but at its host path (`paths_are_host_paths`).
    fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
        if !profile.secret_slots.is_empty() {
            return Err(DriverError::unserved(
                "exec.secret-slots-unserved",
                "Sealed secret slots are not served by the macos adapter.",
                "secret_slots",
            )
            .into());
        }
        if !profile.writable_roots.is_empty() {
            return Err(DriverError::unserved(
                substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
                "Writable roots outside the workspace are not served by the macos adapter.",
                "writable_roots",
            )
            .into());
        }
        if !profile.read_only_files.is_empty() {
            return Err(DriverError::unserved(
                READ_ONLY_FILES_UNSERVED,
                "Single read-only files are not served by the macos adapter.",
                "read_only_files",
            )
            .into());
        }
        if profile.read_only_roots.len() > MAX_READ_ONLY_ROOTS as usize {
            return Err(DriverError::refused(
                READ_ONLY_ROOT_INVALID,
                format!("at most {MAX_READ_ONLY_ROOTS} read-only roots"),
                "read_only_roots",
            )
            .into());
        }
        for root in &profile.read_only_roots {
            if !is_absolute_normal(&root.host_path) {
                return Err(DriverError::refused(
                    READ_ONLY_ROOT_INVALID,
                    format!("{} is not an absolute normal path", root.host_path),
                    "read_only_roots",
                )
                .into());
            }
            if root.mount != root.host_path {
                return Err(DriverError::refused(
                    READ_ONLY_ROOT_INVALID,
                    format!(
                        "paths_are_host_paths: {} can appear only at its host path, not at {}",
                        root.host_path, root.mount
                    ),
                    "read_only_roots",
                )
                .into());
            }
        }
        // A sealed path or an exec subpath the profile cannot carry is refused before anything
        // runs; what each names on the host is checked at the start.
        for path in &profile.sealed_paths {
            seatbelt::relative_literal(path)?;
        }
        for subpath in &profile.exec_subpaths {
            if subpath.starts_with('/') {
                seatbelt::absolute_literal(subpath)?;
            } else {
                seatbelt::relative_literal(subpath)?;
            }
        }
        Ok(())
    }

    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        admitted.check_on(&self.facts, false)?;
        if admitted.profile().private_tmpdir {
            return Err(DriverError::unserved(
                super::RENDER_UNSERVED,
                "A render does not create the private TMPDIR a start creates, so it cannot name it.",
                "render",
            )
            .into());
        }
        let confined = Self::command(admitted, command, None)?;
        Ok(RenderedCommand {
            program: PathBuf::from(confined.get_program()),
            args: confined
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        })
    }

    /// Every refusal, from the contract's recheck to the spawn, writes its audit record.
    async fn start(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        let started = self.start_run(admitted, command);
        admitted.audit_refusal(&self.facts, started)
    }

    async fn observe(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
        let run = self.run(run)?;
        Ok(Self::observation(&run))
    }

    async fn output(&self, run: &RunId) -> Result<RunOutput, ConfinementError> {
        let run = self.run(run)?;
        let captured = run.captured.lock();
        Ok(RunOutput {
            stdout: captured.stdout.clone(),
            stderr: captured.stderr.clone(),
            stdout_truncated: captured.stdout_truncated,
            stderr_truncated: captured.stderr_truncated,
        })
    }

    async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
        let run = self.run(run)?;
        let mut phase = run.phase.clone();
        // The sender lives until the run is reaped, and it always sends before it goes.
        let _ = phase
            .wait_for(|phase| !matches!(phase, Phase::Running))
            .await;
        Ok(Self::observation(&run))
    }

    /// Signals the run's process group. This is not whole-tree kill, which `macos` does not
    /// prove: a child that called `setsid` is not in the group.
    async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        // Held across `kill`: the run's task cannot reap the leader meanwhile, so the group id
        // cannot have been reused by another process. Until it is reaped the run is `Running`,
        // and the signal goes to every member of the group, the leader exited or not.
        let reaped = run.reaped.lock();
        if *reaped {
            return Ok(());
        }
        // SAFETY: `kill` with a negative pid signals one process group; no memory is involved.
        if unsafe { libc::kill(-run.pid, signal.number()) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(DriverError::failed(
                    "exec.signal-failed",
                    format!("macos signal: {error}"),
                )
                .into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use substrate_wire::{ReadOnlyFile, SecretSlotRequest, WritableHostRoot};

    use super::*;
    use crate::confinement::{AuditRecord, AuditSink, Network, ProcessBounds};

    fn adapter() -> MacosAdapter {
        MacosAdapter::new()
    }

    fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink_records = Arc::clone(&records);
        (
            AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
            records,
        )
    }

    fn root(host_path: &str, mount: &str) -> ReadOnlyRoot {
        ReadOnlyRoot {
            host_path: host_path.into(),
            mount: mount.into(),
        }
    }

    fn driver_code(error: &ConfinementError) -> &'static str {
        match error {
            ConfinementError::Driver(error) => error.code,
            other => panic!("expected a driver refusal, got {other}"),
        }
    }

    #[test]
    fn the_declared_set_is_the_final_set() {
        let facts = proof();
        assert_eq!(facts.adapter, AdapterName::Macos);
        assert_eq!(facts.inner, None);
        assert_eq!(
            facts.served_families,
            [
                ContractFamily::ProcessRun,
                ContractFamily::ProcessWatch,
                ContractFamily::OutputRead,
                ContractFamily::Wait,
                ContractFamily::Workspace,
                ContractFamily::FilesystemView,
                ContractFamily::Audit,
            ]
        );
        assert_eq!(
            facts.facts,
            [
                Fact::PathsAreHostPaths,
                Fact::IpcSignalsContained,
                Fact::SealedPaths,
                Fact::ExecSubpaths,
                Fact::PrivateTmpdir,
            ]
        );
        assert!(!facts.declared_insecure());
        for fact in S2_FACTS {
            assert!(
                facts.proves(&fact),
                "{fact:?} is not proved after {S2_STORY}"
            );
        }
        super::super::check_open(&facts).expect("the open check admits the macos proof");
    }

    #[test]
    fn a_build_fact_the_probe_did_not_show_is_not_proved_and_is_refused_by_name() {
        let probed = |sealed_paths, exec_subpaths, private_tmpdir| {
            MacosAdapter::with_facts(proved(BuildProof {
                sealed_paths,
                exec_subpaths,
                private_tmpdir,
            }))
        };
        let cases = [
            (
                probed(false, true, true),
                ConfinementProfile {
                    sealed_paths: vec![".git".into()],
                    ..ConfinementProfile::default()
                },
                Fact::SealedPaths,
            ),
            (
                probed(true, false, true),
                ConfinementProfile {
                    exec_subpaths: vec!["target".into()],
                    ..ConfinementProfile::default()
                },
                Fact::ExecSubpaths,
            ),
            (
                probed(true, true, false),
                ConfinementProfile {
                    private_tmpdir: true,
                    ..ConfinementProfile::default()
                },
                Fact::PrivateTmpdir,
            ),
        ];
        for (adapter, profile, missing) in cases {
            assert!(!adapter.facts().proves(&missing), "{missing:?}");
            assert_eq!(adapter.facts().facts.len(), 4, "{missing:?}");
            match adapter.admit(&profile) {
                Err(ConfinementError::FactUnserved { fact, .. }) => assert_eq!(fact, missing),
                other => panic!("{missing:?}: expected confinement.fact-unserved, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_build_fields_are_admitted() {
        let adapter = adapter();
        for (profile, fact) in [
            (
                ConfinementProfile {
                    sealed_paths: vec![".git/hooks".into()],
                    ..ConfinementProfile::default()
                },
                Fact::SealedPaths,
            ),
            (
                ConfinementProfile {
                    exec_subpaths: vec!["target".into(), "/usr/bin".into()],
                    ..ConfinementProfile::default()
                },
                Fact::ExecSubpaths,
            ),
            (
                ConfinementProfile {
                    private_tmpdir: true,
                    ..ConfinementProfile::default()
                },
                Fact::PrivateTmpdir,
            ),
        ] {
            let (sink, records) = recording();
            let admitted = adapter
                .admit(&ConfinementProfile {
                    audit: Some(sink),
                    ..profile
                })
                .expect("admitted");
            let record = serde_json::to_value(admitted.record()).expect("record");
            assert_eq!(record["facts"], serde_json::json!([fact]), "{record}");
            assert_eq!(records.lock().len(), 1);
        }
    }

    #[test]
    fn an_unrepresentable_build_path_is_refused_at_admission() {
        let adapter = adapter();
        let sealed = |path: &str| ConfinementProfile {
            sealed_paths: vec![path.into()],
            ..ConfinementProfile::default()
        };
        let exec = |path: &str| ConfinementProfile {
            exec_subpaths: vec![path.into()],
            ..ConfinementProfile::default()
        };
        for mut profile in [
            sealed("quo\"te"),
            sealed("/abs/olute"),
            sealed("../up"),
            sealed(""),
            exec("quo\"te"),
            exec("/usr/b\"in"),
            exec("a/../b"),
            exec("/usr//bin"),
        ] {
            let named = format!("{:?}{:?}", profile.sealed_paths, profile.exec_subpaths);
            let (sink, records) = recording();
            profile.audit = Some(sink);
            let error = adapter.admit(&profile).expect_err(&named);
            assert_eq!(
                driver_code(&error),
                seatbelt::PATH_UNREPRESENTABLE,
                "{named}"
            );
            let records = records.lock();
            assert_eq!(records.len(), 1, "{named}");
            assert_eq!(
                serde_json::to_value(&records[0]).expect("record")["event"],
                "profile_refused"
            );
        }
    }

    fn admitted_in(workspace: &Path, profile: ConfinementProfile) -> AdmittedProfile {
        adapter()
            .admit(&ConfinementProfile {
                workspace: Some(workspace.to_path_buf()),
                ..profile
            })
            .expect("admitted")
    }

    #[test]
    fn the_build_policy_seals_execs_and_writes_the_private_tmpdir() {
        let workspace = tempfile::tempdir().expect("workspace");
        let private = tempfile::tempdir().expect("private tmpdir");
        let declared = tempfile::tempdir().expect("root");
        let host_path = declared.path().to_str().expect("UTF-8").to_owned();
        let admitted = admitted_in(
            workspace.path(),
            ConfinementProfile {
                read_only_roots: vec![root(&host_path, &host_path)],
                sealed_paths: vec![".cargo".into(), "deep/sealed".into()],
                exec_subpaths: vec!["target".into(), "/usr/bin".into(), host_path.clone()],
                private_tmpdir: true,
                ..ConfinementProfile::default()
            },
        );
        let policy = MacosAdapter::policy(&admitted, Path::new("/bin/sh"), Some(private.path()))
            .expect("policy")
            .resolved()
            .expect("resolved")
            .profile()
            .expect("profile");
        let ws = std::fs::canonicalize(workspace.path()).expect("canonical");
        let ws = ws.display();
        let private = std::fs::canonicalize(private.path()).expect("canonical");
        let declared = std::fs::canonicalize(declared.path()).expect("canonical");
        let usr_bin = std::fs::canonicalize("/usr/bin").expect("canonical");
        for line in [
            format!("(allow file-write* (subpath \"{}\"))", private.display()),
            format!("(allow process-exec (subpath \"{ws}/target\"))"),
            format!("(allow process-exec (subpath \"{}\"))", usr_bin.display()),
            format!("(allow process-exec (subpath \"{}\"))", declared.display()),
            format!("(deny file-write* (subpath \"{ws}/.cargo\"))"),
            format!("(deny file-write* (literal \"{ws}/deep\"))"),
            format!("(deny file-write* (subpath \"{ws}/deep/sealed\"))"),
        ] {
            assert!(policy.contains(&line), "missing {line}\n{policy}");
        }
    }

    #[test]
    fn a_sealed_path_reached_through_a_symlink_is_refused() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(workspace.path().join("real")).expect("real");
        std::os::unix::fs::symlink("real", workspace.path().join("link")).expect("link");
        for sealed in ["link", "link/hooks"] {
            let admitted = admitted_in(
                workspace.path(),
                ConfinementProfile {
                    sealed_paths: vec![sealed.into()],
                    ..ConfinementProfile::default()
                },
            );
            let error =
                MacosAdapter::policy(&admitted, Path::new("/bin/sh"), None).expect_err(sealed);
            assert_eq!(
                driver_code(&error),
                seatbelt::PATH_UNREPRESENTABLE,
                "{sealed}"
            );
        }
    }

    #[test]
    fn an_exec_subpath_outside_every_declared_location_is_refused() {
        let workspace = tempfile::tempdir().expect("workspace");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let path = elsewhere.path().to_str().expect("UTF-8").to_owned();
        let undeclared = admitted_in(
            workspace.path(),
            ConfinementProfile {
                exec_subpaths: vec![path.clone()],
                ..ConfinementProfile::default()
            },
        );
        let error = MacosAdapter::policy(&undeclared, Path::new("/bin/sh"), None)
            .expect_err("an undeclared exec subpath");
        assert_eq!(driver_code(&error), seatbelt::EXEC_SUBPATH_UNDECLARED);
        let declared = admitted_in(
            workspace.path(),
            ConfinementProfile {
                exec_subpaths: vec![path.clone()],
                read_only_roots: vec![root(&path, &path)],
                ..ConfinementProfile::default()
            },
        );
        MacosAdapter::policy(&declared, Path::new("/bin/sh"), None)
            .expect("an exec subpath inside a declared root");
        let absent = admitted_in(
            workspace.path(),
            ConfinementProfile {
                exec_subpaths: vec![format!("{path}/absent")],
                read_only_roots: vec![root(&path, &path)],
                ..ConfinementProfile::default()
            },
        );
        let error = MacosAdapter::policy(&absent, Path::new("/bin/sh"), None)
            .expect_err("an absent absolute exec subpath");
        assert_eq!(driver_code(&error), seatbelt::PATH_UNREPRESENTABLE);
    }

    #[test]
    fn a_render_of_a_private_tmpdir_is_unserved() {
        let adapter = adapter();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                private_tmpdir: true,
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = adapter
            .render(
                &admitted,
                &Command {
                    argv: vec!["/bin/sh".into()],
                    env: BTreeMap::new(),
                },
            )
            .expect_err("a render cannot name the directory a start creates");
        assert_eq!(driver_code(&error), super::super::RENDER_UNSERVED);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn off_macos_the_adapter_does_not_open() {
        let error =
            super::super::open(super::super::AdapterRequest::Macos(MacosRequest::default()))
                .err()
                .expect("macos is absent off macOS");
        assert!(
            matches!(
                &error,
                ConfinementError::AdapterUnserved {
                    adapter: AdapterName::Macos,
                    ..
                }
            ),
            "{error}"
        );
        assert_eq!(error.code(), super::super::ADAPTER_UNSERVED);
    }

    #[test]
    fn what_seatbelt_cannot_confine_is_refused_by_name_and_audited() {
        let adapter = adapter();
        let bounds = ProcessBounds {
            timeout_ms: 1,
            processes: 1,
            memory_bytes: 1,
            cpu_millis: 1,
        };
        let cases: Vec<(ConfinementProfile, &str, &str)> = vec![
            (
                ConfinementProfile {
                    process_bounds: Some(bounds),
                    ..ConfinementProfile::default()
                },
                super::super::FAMILY_UNSERVED,
                "limits",
            ),
            (
                ConfinementProfile {
                    network: Some(Network::Egress {
                        apertures: vec!["registry".into()],
                    }),
                    ..ConfinementProfile::default()
                },
                super::super::FAMILY_UNSERVED,
                "network_egress",
            ),
            (
                ConfinementProfile {
                    facts: vec![Fact::WholeTreeKill],
                    ..ConfinementProfile::default()
                },
                super::super::FACT_UNSERVED,
                "whole_tree_kill",
            ),
        ];
        for (mut profile, code, named) in cases {
            let (sink, records) = recording();
            profile.audit = Some(sink);
            let error = adapter.admit(&profile).expect_err(named);
            assert_eq!(error.code(), code, "{named}: {error}");
            let records = records.lock();
            assert_eq!(records.len(), 1, "{named}: {records:?}");
            let record = serde_json::to_value(&records[0]).expect("record");
            assert_eq!(record["event"], "profile_refused", "{record}");
            assert_eq!(record["code"], code, "{record}");
            let field = if code == super::super::FAMILY_UNSERVED {
                "family"
            } else {
                "fact"
            };
            assert_eq!(record[field], named, "{record}");
        }
    }

    #[test]
    fn what_the_policy_cannot_carry_is_refused_at_admission() {
        let adapter = adapter();
        let cases: Vec<(ConfinementProfile, &str)> = vec![
            (
                ConfinementProfile {
                    secret_slots: vec![SecretSlotRequest {
                        slot: "token".into(),
                        fd: 3,
                    }],
                    ..ConfinementProfile::default()
                },
                "exec.secret-slots-unserved",
            ),
            (
                ConfinementProfile {
                    writable_roots: vec![WritableHostRoot {
                        host: "cache".into(),
                        mount: "/cache".into(),
                    }],
                    ..ConfinementProfile::default()
                },
                substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
            ),
            (
                ConfinementProfile {
                    read_only_files: vec![ReadOnlyFile {
                        host_path: "/etc/hosts".into(),
                        mount: "/etc/hosts".into(),
                    }],
                    ..ConfinementProfile::default()
                },
                READ_ONLY_FILES_UNSERVED,
            ),
            (
                ConfinementProfile {
                    read_only_roots: vec![root("/usr/share", "/opt/share")],
                    ..ConfinementProfile::default()
                },
                READ_ONLY_ROOT_INVALID,
            ),
            (
                ConfinementProfile {
                    read_only_roots: vec![root("/usr/../etc", "/usr/../etc")],
                    ..ConfinementProfile::default()
                },
                READ_ONLY_ROOT_INVALID,
            ),
            (
                ConfinementProfile {
                    read_only_roots: vec![root("usr/share", "usr/share")],
                    ..ConfinementProfile::default()
                },
                READ_ONLY_ROOT_INVALID,
            ),
            (
                ConfinementProfile {
                    read_only_roots: (0..=MAX_READ_ONLY_ROOTS)
                        .map(|index| {
                            let path = format!("/r/{index}");
                            root(&path, &path)
                        })
                        .collect(),
                    ..ConfinementProfile::default()
                },
                READ_ONLY_ROOT_INVALID,
            ),
        ];
        for (mut profile, code) in cases {
            let (sink, records) = recording();
            profile.audit = Some(sink);
            let error = adapter.admit(&profile).expect_err(code);
            assert_eq!(driver_code(&error), code, "{error}");
            let records = records.lock();
            assert_eq!(records.len(), 1, "{code}: {records:?}");
            assert_eq!(
                serde_json::to_value(&records[0]).expect("record")["code"],
                code
            );
        }
        let admitted = adapter
            .admit(&ConfinementProfile {
                read_only_roots: vec![root("/usr/share", "/usr/share")],
                ..ConfinementProfile::default()
            })
            .expect("a root at its host path is admitted");
        assert!(!admitted.declared_insecure());
    }

    /// Off macOS there is no `sandbox-exec`: a start on an adapter built here, past the open that
    /// would have refused, is still refused by name and nothing runs.
    #[cfg(not(target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn without_the_launcher_a_start_runs_nothing() {
        let adapter = adapter();
        let workspace = tempfile::tempdir().expect("workspace");
        let (sink, records) = recording();
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                audit: Some(sink),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = adapter
            .start(
                &admitted,
                &Command {
                    argv: vec!["/bin/sh".into(), "-c".into(), "printf ran > marker".into()],
                    env: BTreeMap::new(),
                },
            )
            .await
            .expect_err("no launcher");
        assert_eq!(driver_code(&error), seatbelt::SANDBOX_UNAVAILABLE);
        assert!(!workspace.path().join("marker").exists());
        let events: Vec<String> = records
            .lock()
            .iter()
            .map(|record| {
                serde_json::to_value(record).expect("record")["event"]
                    .as_str()
                    .expect("event")
                    .to_owned()
            })
            .collect();
        // The refused start writes its own record (ADR 0109 decision 2).
        assert_eq!(events, ["profile_admitted", "profile_refused"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_relative_program_is_refused_before_anything_runs() {
        let adapter = adapter();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = adapter
            .start(
                &admitted,
                &Command {
                    argv: vec!["sh".into()],
                    env: BTreeMap::new(),
                },
            )
            .await
            .expect_err("relative");
        assert_eq!(driver_code(&error), seatbelt::PATH_UNREPRESENTABLE);
    }
}
