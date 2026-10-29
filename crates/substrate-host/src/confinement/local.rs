//! The `local` adapter: an unconfined host process, declared insecure (ADR 0109 decision 4).
//!
//! It serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace` (a host directory)
//! and `audit`, and nothing that would claim confinement. It proves `declared_insecure`,
//! `paths_are_host_paths`, `host_network`, `stdin_pipe` and `output_live` (pipes, not
//! confinement: design 32), and `local_permitted` unless the host's policy forbids it (F3 A).
//! Of the embedder's signals only `Kill` stops live delivery; a child that traps `Int` or `Term`
//! keeps delivering until it ends or stalls. It is opened only by name, a
//! profile on it needs a reason, and every observation carries that reason. Nothing falls back to
//! it.
//!
//! Declared insecure is not unchecked: a start holds the environment to the host driver's closed,
//! non-secret shape (`exec.environment-refused`), refuses a working directory reached through a
//! symbolic link (`exec.workdir-missing`), and rechecks the adapter's own proof, so a host whose
//! policy forbids `local` never runs a profile admitted elsewhere.
//!
//! A run is `Running` until its leader has exited and every process holding its output has closed
//! it. The leader is left unreaped for all of that time, so its pid, which is the run's process
//! group id, cannot be reused, and a signal reaches every member of the group still there. Then
//! the leader is reaped and the run waits, up to [`LOCAL_GROUP_SETTLE`], for the group to be
//! empty, so a member it killed has been reaped too when `wait` returns. A member that closed its
//! output and outlives that bound is not tracked: once the leader is reaped the group id may come
//! to name another process, and a signal is not sent.

use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use substrate_wire::Signal;
use tokio::sync::watch;

use super::live::{self, LiveOutput};
use super::stdin::{self, StdinWriter};
use super::{
    AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, OutputEvent, RenderedCommand, RunExit, RunId,
    RunObservation, RunOutput, RunState,
};
use crate::DriverError;
use crate::output_retention::{Captured, drain};

/// The host's policy variable (ADR 0109 F3 A). Unset, or `permit`, permits `local`; any other
/// value forbids it, so a misspelt policy fails closed.
pub const LOCAL_POLICY_ENV: &str = "SUBSTRATE_CONFINEMENT_LOCAL";

/// Each output stream is kept up to this many bytes, the ones its retention chooses, and drained
/// past it.
pub const LOCAL_OUTPUT_LIMIT_BYTES: usize = 1_048_576;

/// Runs one adapter tracks; past it, ended runs are forgotten oldest first, and a start with every
/// tracked run still running is refused `exec.tracking-capacity`.
pub const LOCAL_MAX_TRACKED_RUNS: usize = 128;

/// How long a run, its leader reaped and its output closed, waits for the rest of its process
/// group to be gone, killed members reaped by whoever inherited them, before it reports itself
/// ended anyway.
pub const LOCAL_GROUP_SETTLE: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether any process, a zombie included, is still in process group `group`.
fn group_remains(group: i32) -> bool {
    // SAFETY: signal 0 checks existence and permission and delivers nothing; no memory is involved.
    let present = unsafe { libc::kill(-group, 0) } == 0;
    present || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Where the decision to permit `local` comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LocalPolicy {
    /// The host's policy, read from [`LOCAL_POLICY_ENV`] at open.
    #[default]
    FromHost,
    /// Forbidden whatever the host says. An embedder can only tighten the host's policy.
    Refuse,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalRequest {
    pub policy: LocalPolicy,
}

fn host_permits_local() -> bool {
    match std::env::var_os(LOCAL_POLICY_ENV) {
        None => true,
        Some(value) => value == "permit",
    }
}

pub(crate) fn open(request: LocalRequest) -> Arc<dyn ConfinementAdapter> {
    let permitted = request.policy == LocalPolicy::FromHost && host_permits_local();
    Arc::new(LocalAdapter {
        facts: proof(permitted),
        runs: Mutex::new(Vec::new()),
        sequence: Mutex::new(0),
    })
}

/// `local`'s declared set; `systemd` around it starts from the same.
pub fn proof(permitted: bool) -> AdapterFacts {
    AdapterFacts::new(
        AdapterName::Local,
        None,
        &[
            (ContractFamily::ProcessRun, true),
            (ContractFamily::ProcessWatch, true),
            (ContractFamily::OutputRead, true),
            (ContractFamily::Wait, true),
            (ContractFamily::Workspace, true),
            (ContractFamily::Audit, true),
        ],
        &[
            (Fact::DeclaredInsecure, true),
            (Fact::PathsAreHostPaths, true),
            (Fact::LocalPermitted, permitted),
            (Fact::HostNetwork, true),
            (Fact::StdinPipe, true),
            (Fact::OutputLive, true),
        ],
    )
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Running,
    Ended(RunExit),
    Unknown,
}

struct LocalRun {
    id: RunId,
    pid: i32,
    reason: Option<String>,
    phase: watch::Receiver<Phase>,
    captured: Arc<Mutex<Captured>>,
    /// Set, under this lock, once the leader has exited and the run's output has closed, just
    /// before the leader is reaped: while it is false the leader is at least a zombie, so its pid
    /// still names this run's process group.
    reaped: Arc<Mutex<bool>>,
    /// The run's standard input, when its profile piped it (design 32).
    stdin: Option<Arc<StdinWriter>>,
    /// The run's live output, when its profile asked for it (design 32).
    live: Option<Arc<LiveOutput>>,
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

pub struct LocalAdapter {
    facts: AdapterFacts,
    runs: Mutex<Vec<Arc<LocalRun>>>,
    sequence: Mutex<u64>,
}

impl LocalAdapter {
    fn run(&self, id: &RunId) -> Result<Arc<LocalRun>, ConfinementError> {
        self.runs
            .lock()
            .iter()
            .find(|run| &run.id == id)
            .cloned()
            .ok_or_else(super::unknown_run)
    }

    fn observation(run: &LocalRun) -> RunObservation {
        let (state, exit) = match *run.phase.borrow() {
            Phase::Running => (RunState::Running, None),
            Phase::Ended(exit) => (RunState::Ended, Some(exit)),
            Phase::Unknown => (RunState::Unknown, None),
        };
        RunObservation {
            run: run.id.clone(),
            adapter: AdapterName::Local,
            inner: None,
            declared_insecure: run.reason.clone(),
            state,
            exit,
            memory_oom_kills: None,
        }
    }

    /// Makes room for one more run, forgetting ended runs oldest first.
    fn reserve(&self) -> Result<(), ConfinementError> {
        let mut runs = self.runs.lock();
        while runs.len() >= LOCAL_MAX_TRACKED_RUNS {
            let Some(ended) = runs
                .iter()
                .position(|run| !matches!(*run.phase.borrow(), Phase::Running))
            else {
                return Err(DriverError::exhausted(
                    "exec.tracking-capacity",
                    "Every tracked local run is still running.",
                    "run",
                )
                .into());
            };
            runs.remove(ended);
        }
        Ok(())
    }

    /// The directory a start runs in: the workspace, or the workdir beneath it. Every component
    /// of the workdir must be a directory itself, never a symbolic link, so the run starts beneath
    /// the workspace as `linux` does (design 26); one that is not is `exec.workdir-missing`.
    fn directory(admitted: &AdmittedProfile) -> Result<PathBuf, ConfinementError> {
        let workspace = super::workspace_of(admitted)?;
        if !workspace.is_dir() {
            return Err(DriverError::not_found().into());
        }
        let Some(workdir) = &admitted.profile().workdir else {
            return Ok(workspace.clone());
        };
        let relative = super::relative_workdir(workdir)?;
        let missing = || {
            ConfinementError::from(DriverError::refused(
                substrate_wire::EXEC_WORKDIR_MISSING,
                "The working directory is not a directory beneath the workspace.",
                "workdir",
            ))
        };
        let mut directory = workspace.clone();
        for component in relative.components() {
            directory.push(component);
            let entry = std::fs::symlink_metadata(&directory).map_err(|_| missing())?;
            if !entry.file_type().is_dir() {
                return Err(missing());
            }
        }
        Ok(directory)
    }

    /// Spawns on the calling task's Tokio runtime; `start` is its one caller.
    fn start_run(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        admitted.check_on(&self.facts, true)?;
        super::check_command(command)?;
        crate::environment::check(&command.env)?;
        let directory = Self::directory(admitted)?;
        self.reserve()?;
        let mut child = tokio::process::Command::new(&command.argv[0]);
        child
            .args(&command.argv[1..])
            .env_clear()
            .envs(&command.env)
            .current_dir(&directory)
            .stdin(stdin::stdio(admitted.profile().stdin))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group, so a signal reaches what it started and stayed in the group.
            // This is not whole-tree kill, which `local` does not prove.
            .process_group(0);
        let mut child = child.spawn().map_err(|error| {
            DriverError::failed("exec.spawn-failed", format!("local spawn: {error}"))
        })?;
        let pid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .ok_or_else(|| DriverError::failed("exec.spawn-failed", "local spawn: no pid"))?;
        let writer = stdin::child_writer(admitted.profile().stdin, child.stdin.take());
        // The run record owns the writer; the reaper only reaches it, so dropping the adapter
        // drops the pipe and the child reads end of file.
        let ending = writer.as_ref().map(Arc::downgrade);
        let id = {
            let mut sequence = self.sequence.lock();
            *sequence += 1;
            RunId::new(format!("local-{pid}-{sequence}"))
        };
        let captured = Arc::new(Mutex::new(Captured::new(
            admitted.profile().output_retention,
            LOCAL_OUTPUT_LIMIT_BYTES,
        )));
        let (phase, observed) = watch::channel(Phase::Running);
        let reaped = Arc::new(Mutex::new(false));
        let (live, [stdout_feed, stderr_feed]) =
            live::open(admitted.profile().output_delivery, || {
                live::group_kill(pid, Arc::clone(&reaped))
            });
        let stdout = child
            .stdout
            .take()
            .map(|stream| tokio::spawn(drain(stream, Arc::clone(&captured), true, stdout_feed)));
        let stderr = child
            .stderr
            .take()
            .map(|stream| tokio::spawn(drain(stream, Arc::clone(&captured), false, stderr_feed)));
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
            let settle = tokio::time::Instant::now() + LOCAL_GROUP_SETTLE;
            while group_remains(pid) && tokio::time::Instant::now() < settle {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            // The run's end, for its standard input too: a write pending now fails, and a
            // descendant outside the group still holding descriptor 0 no longer holds the writer.
            if let Some(writer) = ending.as_ref().and_then(std::sync::Weak::upgrade) {
                writer.end().await;
            }
            let _ = phase.send(match ended {
                Ok(status) => Phase::Ended(RunExit {
                    code: status.code(),
                    signal: status.signal(),
                }),
                Err(_) => Phase::Unknown,
            });
        });
        self.runs.lock().push(Arc::new(LocalRun {
            id: id.clone(),
            pid,
            reason: admitted.insecure_reason().map(str::to_owned),
            phase: observed,
            captured,
            reaped,
            stdin: writer,
            live,
        }));
        admitted.audit_start(&id, &command.argv[0]);
        Ok(id)
    }
}

#[async_trait]
impl ConfinementAdapter for LocalAdapter {
    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    /// A read-only or scoped workspace, which an unconfined process cannot be kept to, and a
    /// secret slot, which needs a sealed descriptor `local` cannot provide.
    fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
        stdin::check_streams(profile)?;
        super::refuse_workspace_access("local", profile)?;
        if !profile.secret_slots.is_empty() {
            return Err(DriverError::unserved(
                "exec.secret-slots-unserved",
                "Sealed secret slots are not served by the local adapter.",
                "secret_slots",
            )
            .into());
        }
        Ok(())
    }

    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        // A start's refusals, short of spawning: a render never answers a command a start refuses.
        admitted.check_on(&self.facts, false)?;
        super::check_command(command)?;
        crate::environment::check(&command.env)?;
        Self::directory(admitted)?;
        Ok(RenderedCommand {
            program: PathBuf::from(&command.argv[0]),
            args: command.argv[1..].to_vec(),
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
        let output = run.captured.lock().output();
        Ok(output)
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

    /// `Kill` is an imposed end and stops live delivery first; `Int` and `Term` close nothing,
    /// since the child may trap them (design 32 § 3).
    async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        if signal == Signal::Kill
            && let Some(live) = &run.live
        {
            live.close();
        }
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
                    format!("local signal: {error}"),
                )
                .into());
            }
        }
        Ok(())
    }

    async fn write_stdin(&self, run: &RunId, bytes: &[u8]) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        stdin::piped(run.stdin.as_ref())?.write(bytes).await
    }

    async fn close_stdin(&self, run: &RunId) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        stdin::piped(run.stdin.as_ref())?.close().await;
        Ok(())
    }

    async fn read_output(
        &self,
        run: &RunId,
        timeout: Duration,
    ) -> Result<Option<OutputEvent>, ConfinementError> {
        let run = self.run(run)?;
        live::live(run.live.as_ref())?
            .read(timeout, || run.captured.lock().extent())
            .await
    }

    fn output_stalled(&self, run: &RunId) -> bool {
        self.run(run)
            .is_ok_and(|run| run.live.as_ref().is_some_and(|live| live.stalled()))
    }
}
