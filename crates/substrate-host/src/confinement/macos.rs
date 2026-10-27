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
//! `sealed_paths`, `exec_subpaths` and `private_tmpdir` are not proved until
//! `story:macos-seatbelt-policy-runs-cargo-test`. A profile needing any of them is refused by its
//! name before anything runs.
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
use crate::seatbelt::{self, SeatbeltPolicy};

/// Each output stream is kept up to this many bytes and drained past it.
pub const MACOS_OUTPUT_LIMIT_BYTES: usize = 1_048_576;

/// Runs one adapter tracks; past it, ended runs are forgotten oldest first, and a start with every
/// tracked run still running is refused `exec.tracking-capacity`.
pub const MACOS_MAX_TRACKED_RUNS: usize = 128;

/// The story that makes this adapter prove [`S2_FACTS`].
pub const S2_STORY: &str = "story:macos-seatbelt-policy-runs-cargo-test";

/// The facts the final `macos` declared set holds and this adapter does not prove yet: each is
/// refused `confinement.fact-unserved` until [`S2_STORY`].
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
    Ok(Arc::new(MacosAdapter::new()))
}

/// `macos`'s declared set.
pub fn proof() -> AdapterFacts {
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
            (Fact::SealedPaths, false),
            (Fact::ExecSubpaths, false),
            (Fact::PrivateTmpdir, false),
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
    /// Set, under this lock, after the leader exited and before it is reaped: while it is false
    /// the leader is at least a zombie, so its pid still names this run's process group.
    leader_exited: Arc<Mutex<bool>>,
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
    fn new() -> Self {
        Self {
            facts: proof(),
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

    /// The confined command one start would spawn: the Seatbelt launcher, its profile, the
    /// program and its arguments, in the workspace or the workdir beneath it. Nothing is spawned.
    fn command(
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<std::process::Command, ConfinementError> {
        super::check_command(command)?;
        let program = Path::new(&command.argv[0]);
        if !program.is_absolute() {
            return Err(DriverError::refused(
                seatbelt::PATH_UNREPRESENTABLE,
                "The program must be an absolute path: a confined run has no search path.",
                "argv",
            )
            .into());
        }
        let workspace = super::workspace_of(admitted)?;
        if !workspace.is_dir() {
            return Err(DriverError::not_found().into());
        }
        let canonical_workspace = canonical(workspace, "workspace")?;
        let profile = admitted.profile();
        let mut policy = SeatbeltPolicy::new(&canonical_workspace).executable(program);
        for root in &profile.read_only_roots {
            policy = policy.read_only(read_only_root(root, &canonical_workspace)?);
        }
        let directory = match &profile.workdir {
            None => canonical_workspace,
            Some(workdir) => {
                let directory = workspace.join(super::relative_workdir(workdir)?);
                let missing = || {
                    ConfinementError::from(DriverError::refused(
                        substrate_wire::EXEC_WORKDIR_MISSING,
                        "The working directory is not a directory beneath the workspace.",
                        "workdir",
                    ))
                };
                let directory = std::fs::canonicalize(&directory).map_err(|_| missing())?;
                if !directory.is_dir() || !directory.starts_with(&canonical_workspace) {
                    return Err(missing());
                }
                directory
            }
        };
        let mut confined = policy.command(program, &command.argv[1..])?;
        confined
            .envs(&command.env)
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
        Ok(())
    }

    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        admitted.check_on(&self.facts, false)?;
        let confined = Self::command(admitted, command)?;
        Ok(RenderedCommand {
            program: PathBuf::from(confined.get_program()),
            args: confined
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        })
    }

    async fn start(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        admitted.check_on(&self.facts, true)?;
        let confined = Self::command(admitted, command)?;
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
        let leader_exited = Arc::new(Mutex::new(false));
        let exited = Arc::clone(&leader_exited);
        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || await_exit_unreaped(pid)).await;
            *exited.lock() = true;
            let ended = child.wait().await;
            for reader in [stdout, stderr].into_iter().flatten() {
                let _ = reader.await;
            }
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
            leader_exited,
        }));
        admitted.audit_start(&id, &command.argv[0]);
        Ok(id)
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
        // cannot have been reused by another process.
        let exited = run.leader_exited.lock();
        if *exited {
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
    fn the_declared_set_is_s1s_set() {
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
            [Fact::PathsAreHostPaths, Fact::IpcSignalsContained]
        );
        assert!(!facts.declared_insecure());
        for fact in S2_FACTS {
            assert!(
                !facts.proves(&fact),
                "{fact:?} is proved before {S2_STORY} flips it"
            );
        }
        super::super::check_open(&facts).expect("the open check admits the macos proof");
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
            (
                ConfinementProfile {
                    sealed_paths: vec![".git".into()],
                    ..ConfinementProfile::default()
                },
                super::super::FACT_UNSERVED,
                "sealed_paths",
            ),
            (
                ConfinementProfile {
                    exec_subpaths: vec!["bin".into()],
                    ..ConfinementProfile::default()
                },
                super::super::FACT_UNSERVED,
                "exec_subpaths",
            ),
            (
                ConfinementProfile {
                    private_tmpdir: true,
                    ..ConfinementProfile::default()
                },
                super::super::FACT_UNSERVED,
                "private_tmpdir",
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
        assert_eq!(events, ["profile_admitted"]);
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
