//! The `linux` adapter: the existing [`HostDriver`](crate::HostDriver) — bubblewrap, namespaces,
//! cgroup v2, seccomp and `openat2` — behind the confinement contract.
//!
//! Its families and facts are read from the capability snapshot the driver's probe proved at open,
//! and a family whose proving fact is absent is not served:
//!
//! | served | when the snapshot publishes |
//! |---|---|
//! | `process_run`, `process_watch`, `wait` | `exec.argv-only` and `exec.namespaces` (the confinement floor) |
//! | `output_read` | the floor and `exec.output-limit-bytes` |
//! | `workspace` | `workspace.guarded-io` |
//! | `filesystem_view` | `exec.read-only-roots` and `exec.read-only-files` |
//! | `network_egress` | `exec.egress-apertures` (an operator-declared aperture, proved) |
//! | `limits` | `exec.cgroup-limits` with processes, memory and CPU |
//! | `audit` | always: the adapter writes its records itself |
//!
//! | proved | when |
//! |---|---|
//! | `whole_tree_kill` | `exec.cgroup-kill` |
//! | `ipc_socket_allowlist` | the floor: every exec installs ADR 0039's seccomp socket filter or does not start |
//! | `ipc_signals_contained` | the floor with a pid namespace |
//! | `stdin_pipe`, `output_live` | the floor: the session path's pipes, through bubblewrap |
//! | `exec_subpaths` | the floor, and the driver's probe held a run to an allow-list with the helper `HostConfig.exec_gate` names ([`ExecListProof`], design 35). Not where the open withheld it for a path on a file-system type other than `ext4`, `overlay` and `tmpfs`: the proof's `withheld` then says which path and type (D27) |
//! | `exec_denied_paths` | `exec_subpaths`, and a second probe held a run to both lists, with one deny entry naming its file only through a second mount and one naming a file in a mount beneath a listed directory (design 35 § 9, D30) |
//!
//! **`exec_denied_paths` on `linux`** (design 35 § 8). A denied path gets no execute right: not
//! from a listed directory that holds it, not as the loader and not as the program, and a start
//! whose program is denied is refused `exec.sandbox-program-unlisted` before dispatch. Landlock
//! has no deny rule, so the helper grants each entry of such a directory on its own. Whether one
//! path holds another is decided by place, so the file is denied under every mount of its
//! directory and under a symbolic link; a hard link that existed before the start in another
//! listed directory runs, and so does a copy in a listed directory the run can write. An entry
//! may be absent at the start. **In a listed directory a run can write, a deny entry holds for
//! the files that exist at the start, under the names they have then**: a directory inside an
//! expanded directory that holds no denied path gets one rule, what the run creates in it
//! executes, and the rule moves with a rename, so a file the run creates or moves can run
//! under any name, the denied name included. Nothing runs that the run could not already run
//! by copying it into a listed directory it can write, and a deny-list is exact only where
//! the run cannot write. A deny-list beside an empty `exec_subpaths` is refused at admission,
//! `confinement.exec-denied-paths-without-subpaths`, on every adapter.
//!
//! **A deny entry holds across the mounts** (D30). What is mounted beneath a denied directory
//! is denied with it, and a listed directory with a denied place in a mount beneath it is
//! expanded. The driver refuses a program that lies in what a deny entry reaches on its own
//! mount table. Where a mount beneath a denied directory, or beneath a listed directory of a
//! start with a deny-list, cannot be compared (it is covered, or its root was removed), the
//! start is refused `exec.sandbox-place-unserved` before dispatch; so it is for a mount of a
//! type outside `ext4`, `overlay` and `tmpfs` beneath a denied directory. A start with an
//! allow-list alone asks none of this.
//!
//! **`exec_subpaths` on `linux`** (design 35). A profile with a non-empty list is held to its
//! literal program and the list: a Landlock ruleset, applied inside the sandbox by
//! `substrate-exec-gate` as the last program before the run's own, lets `execve` succeed only
//! for the program, the host's ELF loader and what the entries name. **An empty list runs every
//! program of the view, as before**; on `macos` an empty list runs only the program. That
//! difference is stated, not removed.
//!
//! | an entry | on `linux` |
//! |---|---|
//! | names | a directory, which admits every file beneath it, or one file |
//! | is written | in the run's view, not in host paths: relative beneath `/workspace`, absolute beneath `/usr`, `/bin`, `/lib`, `/lib64`, `/workspace` or the mount of a declared root or file |
//! | anywhere else (`/tmp`, `/proc`, `/dev`, `/etc`) | `exec.sandbox-exec-subpath-undeclared` |
//! | not normal, or holding a quote, backslash or control character | `exec.sandbox-path-unrepresentable`, at admission |
//! | absent at the start | `exec.sandbox-path-unrepresentable`: a rule is tied to an open file, not to a name |
//! | with a symbolic link in a mount that shows something a run can write | `exec.sandbox-path-unrepresentable` |
//! | resolving through a link into a mount the run can write | `exec.sandbox-path-unrepresentable` |
//!
//! The program is an absolute view path when the list is not empty. Every one of these refusals
//! is made before anything is dispatched, and writes its audit record.
//!
//! What the list does not stop: it restricts which files `execve` accepts, not what an allowed
//! program does. An allowed interpreter runs a script outside the list, the loader run as a
//! program loads one, and a program can map code it wrote. The run's variables are not
//! filtered: on a start with a list they reach the run's program and no program before it
//! (D28), and an object that `LD_PRELOAD` names is loaded into every allowed dynamic program
//! and runs its code there, while an `execve` it makes of an unlisted path fails. A listed
//! directory does not reach
//! through a second mount of a part of it: with `/usr` listed, `/bin/sh` is denied where `/bin`
//! is a mount of `/usr/bin`, so a profile lists `/usr/bin`, the directory that holds the
//! programs. A rename or link into a listed directory fails with `EXDEV`, and a listed directory
//! removed and created again is no longer listed.
//!
//! A live run's output is the session path's queue (design 32 § 5). `read_output` reads it in
//! slices of at most 100 ms so that a `signal`, which takes the same receiver lock
//! the host holds for one read, waits at most one slice; every signal is end-equivalent, since
//! the host closes the queue, signals and kills the cgroup.
//!
//! `network_ingress` is served by no adapter. `paths_are_host_paths`, `sealed_paths`,
//! `private_tmpdir` and `host_network` are not proved, and neither is `exec_subpaths` or
//! `exec_denied_paths` on a host
//! without a configured helper or whose probe did not hold; a profile requiring one is refused
//! `confinement.fact-unserved`.
//!
//! Some fields need more of the snapshot than their family's row, and admission refuses them by
//! name when the snapshot lacks it, so a start never meets a fact admission could have read:
//!
//! | field | needs | refused |
//! |---|---|---|
//! | `workspace_access` other than `ReadWrite` | `exec.workspace-scoped-write` | `exec.workspace-scoped-write-unserved` |
//! | `writable_roots` | `exec.writable-roots.host` | `confinement.family-unserved` naming `filesystem_view` |
//! | a writable root's `host` | that name in `exec.writable-roots.host` | `exec.writable-roots-host-unserved` |
//! | `workdir` | `exec.workdir` | `exec.workdir-unserved` |
//! | `network: Egress` names | each name in `exec.egress-apertures` | `exec.aperture-undeclared` |
//! | `secret_slots` | `secrets.slots` | `exec.secret-slots-unserved` |
//! | a slot's name | that name in `secrets.slots` | `exec.secret-slot-unknown` |
//! | an `exec_subpaths` entry | a text every adapter can carry | `exec.sandbox-path-unrepresentable` |
//!
//! Before any of them, a workspace access meets the rules every adapter shares: the wire's shape
//! (`exec.workspace-access-invalid`) and no writable subtree at or beneath `.git`
//! (`confinement.writable-subtree-git`). A subtree that is absent, a symlink or not a directory
//! is refused when the start opens the workspace, by the driver's `exec.workspace-subtree-*`
//! names. So is a subtree that is, or holds, a submodule the workspace's `.gitmodules` registers,
//! read through the pinned workspace descriptor: `confinement.writable-subtree-git`, as `.git`
//! itself. `output_retention` is served wherever `output_read` is: the driver keeps the bytes it
//! names within the same bound, and writes its truncation marker only under `Head`.
//!
//! Every refusal of a start, by the contract's recheck, the adapter or the driver, writes its
//! audit record.

use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(not(target_os = "linux"))]
use super::AdapterName;
use super::{ConfinementAdapter, ConfinementError};
use crate::HostConfig;

/// What opening `linux` takes: the host driver's configuration. Its `cgroup_root` is the
/// delegated cgroup v2 root runs are bounded in; `systemd` sets it to its scope.
#[derive(Debug, Clone)]
pub struct LinuxRequest {
    pub config: HostConfig,
}

impl LinuxRequest {
    pub fn new(config: HostConfig) -> Self {
        Self { config }
    }

    /// The delegated cgroup root, which becomes `HostConfig.cgroup_root`.
    #[must_use]
    pub fn with_cgroup_root(mut self, cgroup_root: Option<PathBuf>) -> Self {
        self.config.cgroup_root = cgroup_root;
        self
    }
}

/// What a host driver's probe proved of the executable lists at open (design 35 D14): the second
/// argument of [`proof`].
///
/// Host-private. The capability snapshot publishes none of it, and its digest binds it: the
/// value is part of what the snapshot identity is computed over.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ExecListProof {
    /// One throwaway run under the helper `HostConfig.exec_gate` names executed what its
    /// allow-list named and nothing else, and linked a file between two unlisted directories.
    pub subpaths: bool,
    /// A second throwaway run, with a deny entry that names a file only through a second mount
    /// of a part of the listed directory, executed what the list allowed and not the denied
    /// file, under its listed name and under a hard link beside it; and, with a second deny
    /// entry in a mount beneath another listed directory, not that file and its neighbour
    /// there (design 35 § 9, D30). Proved only where `subpaths` was.
    pub denied_paths: bool,
    /// Why the open withheld the executable-list facts, where it did (design 35 D27): one
    /// entry per fact and path. Empty where no helper is configured, where every place of
    /// the open was established, and where a probe did not hold. [`proof`] copies it into
    /// [`AdapterFacts::withheld`](super::AdapterFacts::withheld) and proves no fact it names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub withheld: Vec<super::FactWithheld>,
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn open(
    _request: LinuxRequest,
) -> Result<std::sync::Arc<dyn ConfinementAdapter>, ConfinementError> {
    super::unserved(AdapterName::Linux, "the linux adapter needs Linux")
}

#[cfg(target_os = "linux")]
pub(crate) fn open(request: LinuxRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    imp::open(request)
}

#[cfg(target_os = "linux")]
pub use imp::proof;

/// The adapter itself, for `systemd` around `linux`, whose task awaits the end of a run of it
/// (design 36 D20).
#[cfg(target_os = "linux")]
pub(crate) use imp::LinuxAdapter;

/// [`open`], returning the adapter itself.
#[cfg(target_os = "linux")]
pub(crate) fn open_adapter(request: LinuxRequest) -> Result<Arc<LinuxAdapter>, ConfinementError> {
    imp::open_adapter(request)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{BTreeSet, HashMap};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use substrate_wire::{
        CapabilityFacts, CapabilitySnapshot, ConfinementRequest, ExecEnvironment, ExecLimits,
        ExecSignalInput, ExecStartInput, ExecState, NetworkMode, SandboxProfile, Signal,
        WritableRoot,
    };

    use super::super::live::{self, Progress};
    use super::super::stdin::{self, StdinPipe, StdinWriter};
    use super::super::{
        AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
        ConfinementProfile, ContractFamily, Fact, KeptOutput, Network, OutputEvent, ProcessBounds,
        RenderedCommand, RunExit, RunId, RunLimit, RunObservation, RunOutput, RunState, StdinMode,
    };
    use super::{ExecListProof, LinuxRequest};
    use crate::process::ExecLists;
    use crate::{DispatchOutcome, Driver as _, DriverError, HostDriver, PLATFORM_UNSERVED};

    /// The bounds a run gets when its profile states none: the driver bounds every run.
    pub const DEFAULT_BOUNDS: ProcessBounds = ProcessBounds {
        timeout_ms: 600_000,
        processes: 256,
        memory_bytes: 1 << 30,
        cpu_millis: 600_000,
    };

    /// How often `wait` reads the driver's observation.
    const WAIT_POLL: Duration = Duration::from_millis(20);

    /// The longest one host read of a live run waits: the host's `signal` takes the receiver
    /// lock that read holds, so a signal waits at most this long for it (design 32 § 3).
    const READ_SLICE: Duration = Duration::from_millis(100);

    pub(super) fn open(
        request: LinuxRequest,
    ) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
        Ok(open_adapter(request)?)
    }

    pub(super) fn open_adapter(
        request: LinuxRequest,
    ) -> Result<Arc<LinuxAdapter>, ConfinementError> {
        let driver = HostDriver::open(request.config).map_err(|error| {
            if error.code == PLATFORM_UNSERVED {
                ConfinementError::AdapterUnserved {
                    adapter: AdapterName::Linux,
                    detail: error.message,
                }
            } else {
                ConfinementError::Driver(error)
            }
        })?;
        let machine = driver.machine();
        Ok(Arc::new(LinuxAdapter {
            facts: proof(&machine, &driver.exec_list_proof()),
            capability: machine.facts,
            snapshot: machine.snapshot,
            driver,
            writers: parking_lot::Mutex::new(HashMap::new()),
            readers: parking_lot::Mutex::new(HashMap::new()),
        }))
    }

    /// The families and facts a host driver's capability snapshot proves, and of the executable
    /// lists what its probe proved beside the snapshot (design 35 D14): the snapshot's own
    /// `confinement.exec-subpaths` is the published wire fact and stays absent. What the open
    /// withheld is copied into the proof, and a fact an entry names is not proved (D27).
    pub fn proof(machine: &CapabilitySnapshot, exec_lists: &ExecListProof) -> AdapterFacts {
        let facts = &machine.facts;
        let floor = facts.exec_argv_only == Some(true) && facts.exec_namespaces.is_some();
        let limits = facts
            .exec_cgroup_limits
            .as_ref()
            .is_some_and(|limits| limits.processes && limits.memory && limits.cpu);
        let withheld = |fact: &Fact| exec_lists.withheld.iter().any(|entry| entry.fact == *fact);
        let mut proof = AdapterFacts::new(
            AdapterName::Linux,
            None,
            &[
                (ContractFamily::ProcessRun, floor),
                (ContractFamily::ProcessWatch, floor),
                (
                    ContractFamily::OutputRead,
                    floor && facts.exec_output_limit_bytes.is_some(),
                ),
                (ContractFamily::Wait, floor),
                (
                    ContractFamily::Workspace,
                    facts.workspace_guarded_io == Some(true),
                ),
                (
                    ContractFamily::FilesystemView,
                    facts.exec_read_only_roots.is_some() && facts.exec_read_only_files.is_some(),
                ),
                (
                    ContractFamily::NetworkEgress,
                    facts.exec_egress_apertures.is_some(),
                ),
                (ContractFamily::Limits, limits),
                (ContractFamily::Audit, true),
            ],
            &[
                (Fact::WholeTreeKill, facts.exec_cgroup_kill == Some(true)),
                (Fact::IpcSocketAllowlist, floor),
                (Fact::StdinPipe, floor),
                (Fact::OutputLive, floor),
                (
                    Fact::IpcSignalsContained,
                    floor
                        && facts
                            .exec_namespaces
                            .as_ref()
                            .is_some_and(|namespaces| namespaces.pid),
                ),
                // The helper runs inside the floor's sandbox, so without the floor there is
                // nothing it was proved in.
                (
                    Fact::ExecSubpaths,
                    floor && exec_lists.subpaths && !withheld(&Fact::ExecSubpaths),
                ),
                (
                    Fact::ExecDeniedPaths,
                    floor
                        && exec_lists.subpaths
                        && exec_lists.denied_paths
                        && !withheld(&Fact::ExecSubpaths)
                        && !withheld(&Fact::ExecDeniedPaths),
                ),
            ],
        );
        proof.withheld.clone_from(&exec_lists.withheld);
        proof
    }

    pub struct LinuxAdapter {
        facts: AdapterFacts,
        /// The driver's capability facts as probed at open: what admission reads beyond families.
        capability: CapabilityFacts,
        snapshot: String,
        driver: Arc<HostDriver>,
        /// The writer of each run started with a piped standard input, kept while the driver
        /// still tracks the run, so a write after its end is `session.input-closed`.
        writers: parking_lot::Mutex<HashMap<RunId, Arc<StdinWriter>>>,
        /// Each live run's reader lock and where its reader is, kept while the driver still
        /// tracks the run (design 32 § 3).
        readers: parking_lot::Mutex<HashMap<RunId, Arc<tokio::sync::Mutex<Progress>>>>,
    }

    /// A run's standard input under `linux`: the host driver's pipe, by exec id. The host never
    /// drops it at the run's end, so the adapter closes it then (design 32 § 2).
    struct HostPipe {
        driver: Arc<HostDriver>,
        id: String,
    }

    #[async_trait]
    impl StdinPipe for HostPipe {
        async fn write(&mut self, bytes: &[u8]) -> Result<(), ConfinementError> {
            Ok(self.driver.write_pipe_session(&self.id, bytes).await?)
        }

        async fn close(&mut self) {
            let _ = self.driver.close_pipe_session_input(&self.id).await;
        }

        /// Closing is async here: started on the current runtime, where there is one. Where there
        /// is none, the next write or close closes it, and so does the run's end.
        fn close_now(&mut self) {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let driver = Arc::clone(&self.driver);
                let id = self.id.clone();
                runtime.spawn(async move {
                    let _ = driver.close_pipe_session_input(&id).await;
                });
            }
        }
    }

    impl LinuxAdapter {
        /// The writer of `run`; for a run without one, `session.not-pipe` while the driver tracks
        /// it and the driver's own refusal once it does not.
        fn writer(&self, run: &RunId) -> Result<Arc<StdinWriter>, ConfinementError> {
            if let Some(writer) = self.writers.lock().get(run) {
                return Ok(Arc::clone(writer));
            }
            self.driver.processes.observe(run.as_str())?;
            Err(stdin::not_pipe())
        }

        /// The run's end, observed: its standard input is closed and ended (design 32 § 2).
        async fn ended(&self, run: &RunId) {
            let writer = self.writers.lock().get(run).cloned();
            if let Some(writer) = writer
                && !writer.ended()
            {
                writer.end().await;
            }
        }

        /// Forgets the writers of ended runs the driver no longer tracks.
        fn forget_writers(&self) {
            self.writers.lock().retain(|run, writer| {
                !writer.ended() || self.driver.processes.observe(run.as_str()).is_ok()
            });
            // A live run's events stay readable until the driver forgets the run.
            self.readers
                .lock()
                .retain(|run, _| self.driver.processes.observe(run.as_str()).is_ok());
        }

        fn reader(
            &self,
            run: &RunId,
        ) -> Result<Arc<tokio::sync::Mutex<Progress>>, ConfinementError> {
            if let Some(reader) = self.readers.lock().get(run) {
                return Ok(Arc::clone(reader));
            }
            self.driver.processes.observe(run.as_str())?;
            Err(live::not_live())
        }

        /// The next live event under the run's reader lock, reading the host's queue in slices
        /// of at most [`READ_SLICE`] until the caller's deadline: an elapsed slice is not an
        /// answer (design 32 § 3).
        async fn read_live(
            &self,
            run: &RunId,
            timeout: Duration,
        ) -> Result<Option<OutputEvent>, ConfinementError> {
            let reader = self.reader(run)?;
            if timeout.is_zero() {
                return Err(live::timeout_invalid());
            }
            let deadline = tokio::time::Instant::now() + timeout;
            let mut progress = tokio::time::timeout_at(deadline, reader.lock())
                .await
                .map_err(|_| live::read_timeout())?;
            while !progress.exhausted() {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return Err(live::read_timeout());
                }
                match self
                    .driver
                    .read_pipe_session(run.as_str(), left.min(READ_SLICE))
                    .await
                {
                    Ok(Some(frame)) => return Ok(Some(progress.chunk(frame.stream, frame.bytes))),
                    Ok(None) => progress.exhaust(),
                    Err(error) if error.code == substrate_wire::SESSION_READ_TIMEOUT => {}
                    Err(error) => return Err(error.into()),
                }
            }
            let output = self.output(run).await?;
            Ok(progress.after_queue(
                live::record_of(&output),
                self.driver.output_stalled(run.as_str()),
            ))
        }

        /// The workspace's root name: it must be a directory directly beneath the driver's
        /// workspace root, which is where the driver resolves and pins it.
        fn root_name(&self, workspace: &Path) -> Result<String, ConfinementError> {
            let escape = || {
                ConfinementError::Driver(DriverError::refused(
                    "workspace.path-escape",
                    "The linux adapter's workspace must be a directory directly beneath its \
                     workspace root.",
                    "workspace",
                ))
            };
            let root = std::fs::canonicalize(self.driver.root()).map_err(|_| escape())?;
            let parent = workspace
                .parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
                .ok_or_else(escape)?;
            let name = workspace
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(escape)?;
            if parent != root {
                return Err(escape());
            }
            self.driver
                .workspace_path(name)
                .map_err(ConfinementError::Driver)?;
            Ok(name.to_owned())
        }

        fn exec_input(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
            root_name: &str,
        ) -> Result<ExecStartInput, ConfinementError> {
            let profile = admitted.profile();
            let bounds = profile.process_bounds.unwrap_or(DEFAULT_BOUNDS);
            let output_bytes = profile
                .io_bounds
                .map_or(self.driver.config.output_limit_bytes, |io| io.output_bytes);
            let (network, aperture, apertures) = match &profile.network {
                Some(Network::Egress { apertures }) if apertures.len() == 1 => (
                    NetworkMode::Aperture,
                    apertures.first().cloned(),
                    Vec::new(),
                ),
                Some(Network::Egress { apertures }) => {
                    (NetworkMode::Aperture, None, apertures.clone())
                }
                Some(Network::Isolated) | None => (NetworkMode::None, None, Vec::new()),
            };
            let workdir = match &profile.workdir {
                Some(workdir) => Some(format!(
                    "{}/{}",
                    substrate_wire::EXEC_WORKSPACE_MOUNT,
                    super::super::relative_workdir(workdir)?.display()
                )),
                None => None,
            };
            Ok(ExecStartInput {
                workspace: root_name.to_owned(),
                argv: command.argv.clone(),
                env: ExecEnvironment {
                    allow: Vec::new(),
                    set: command.env.clone(),
                },
                sandbox: ConfinementRequest {
                    requires: None,
                    capability_snapshot: self.snapshot.clone(),
                    network,
                    aperture,
                    apertures,
                    profile: SandboxProfile::Workspace,
                    required: true,
                    exec_subpaths: None,
                    exec_denied_paths: Vec::new(),
                },
                limits: ExecLimits {
                    timeout_ms: bounds.timeout_ms,
                    output_bytes,
                    processes: bounds.processes,
                    memory_bytes: bounds.memory_bytes,
                    cpu_millis: bounds.cpu_millis,
                },
                wait: false,
                workspace_access: profile.workspace_access.clone(),
                scratch: None,
                measurements: BTreeSet::new(),
                read_only_roots: profile.read_only_roots.clone(),
                read_only_files: profile.read_only_files.clone(),
                secret_slots: profile.secret_slots.clone(),
                capsule: None,
                lease_ttl_ms: None,
                workdir,
                writable_roots: profile
                    .writable_roots
                    .iter()
                    .cloned()
                    .map(WritableRoot::Host)
                    .collect(),
            })
        }

        async fn start_run(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RunId, ConfinementError> {
            admitted.check_on(&self.facts, true)?;
            super::super::check_command(command)?;
            let workspace = super::super::workspace_of(admitted)?;
            let root_name = self.root_name(workspace)?;
            let input = self.exec_input(admitted, command, &root_name)?;
            let id = format!(
                "ex_{}",
                ulid::Ulid::generate().to_string().to_ascii_lowercase()
            );
            let retention = admitted.profile().output_retention;
            let piped = admitted.profile().stdin;
            let live = admitted
                .profile()
                .output_delivery
                .queued_chunks()
                .map(|queued| usize::try_from(queued).unwrap_or(usize::MAX));
            self.forget_writers();
            match self
                .driver
                .start_exec_for_adapter(
                    &id,
                    &root_name,
                    &input,
                    retention,
                    piped.is_piped(),
                    live,
                    // Every entry of the admitted profile's lists (design 35 D22).
                    ExecLists::of(admitted),
                )
                .await
            {
                DispatchOutcome::Observed(_) => {
                    let run = RunId::new(id.clone());
                    if live.is_some() {
                        self.readers.lock().insert(
                            run.clone(),
                            Arc::new(tokio::sync::Mutex::new(Progress::default())),
                        );
                    }
                    if let StdinMode::Piped { limit_bytes } = piped {
                        let writer = StdinWriter::new(
                            limit_bytes,
                            Box::new(HostPipe {
                                driver: Arc::clone(&self.driver),
                                id: id.clone(),
                            }),
                        );
                        // The adapter's table owns the writer; the task only reaches it, so
                        // dropping the adapter drops the writer, which closes the pipe.
                        let ending = Arc::downgrade(&writer);
                        self.writers.lock().insert(run.clone(), writer);
                        let driver = Arc::clone(&self.driver);
                        tokio::spawn(async move {
                            driver.processes.wait_ended(&id).await;
                            if let Some(writer) = ending.upgrade() {
                                writer.end().await;
                            }
                        });
                    }
                    let end = admitted.audit_start(&run, &command.argv[0]);
                    // A task of its own per run that has a sink, and none without (design 36
                    // D19, D20): it is spawned after `run_started`, so it holds the value itself.
                    if !end.is_inert() {
                        let ended = self.ended_observation(&run);
                        tokio::spawn(async move {
                            // An exec the driver no longer tracks leaves the value unwritten,
                            // and its drop writes `unknown` (D22).
                            if let Some(observation) = ended.await {
                                end.write(observation).finished().await;
                            }
                        });
                    }
                    Ok(run)
                }
                DispatchOutcome::NotDispatched(error)
                | DispatchOutcome::ContainedAbsent(error)
                | DispatchOutcome::OutcomeUnknown(error) => Err(ConfinementError::Driver(error)),
            }
        }

        /// The run's end as its own observation reads it, once the driver published the exec's
        /// end (design 36 D20): it awaits `ProcessRuntime::wait_ended` and observes the exec once.
        /// `None` for an exec the driver does not track. The future holds the driver, not the
        /// adapter, so dropping the adapter loses no record (D21).
        pub(crate) fn ended_observation(
            &self,
            run: &RunId,
        ) -> impl std::future::Future<Output = Option<RunObservation>> + Send + use<> {
            let driver = Arc::clone(&self.driver);
            let run = run.clone();
            async move {
                driver.processes.wait_ended(run.as_str()).await;
                let observed = driver.observe_exec(run.as_str()).await.ok()?;
                let observation = Self::observation(&run, &observed);
                (observation.state != RunState::Running).then_some(observation)
            }
        }

        fn observation(run: &RunId, observed: &crate::ExecObservation) -> RunObservation {
            let exec = &observed.resource;
            let state = match exec.state {
                ExecState::Accepted | ExecState::Running => RunState::Running,
                ExecState::Exited | ExecState::Cancelled | ExecState::Expired => RunState::Ended,
                ExecState::Unknown => RunState::Unknown,
            };
            RunObservation {
                run: run.clone(),
                adapter: AdapterName::Linux,
                inner: None,
                declared_insecure: None,
                state,
                exit: exec.exit.as_ref().map(|exit| RunExit {
                    code: exit.code.map(i32::from),
                    signal: exit.signal.map(Signal::number),
                }),
                limit: if state == RunState::Running {
                    None
                } else {
                    ended_limit(
                        exec.refusal.as_ref().map(|refusal| refusal.code.as_str()),
                        observed.end_of_run,
                    )
                },
                memory_oom_kills: observed.end_of_run.and_then(|end| end.memory_oom_kills),
                // The ended count; `observe` reads a running run's own (design 36 D13).
                process_limit_hits: observed.end_of_run.and_then(|end| end.process_limit_hits),
            }
        }
    }

    /// The limit that ended an exec: the one its refusal code names, except a stall the driver
    /// flagged after the child exited on its own, which ended nothing and whose refusal stays on
    /// the record (design 36 D5). Such an exec names what ended it, in D4's order: the aperture
    /// ceiling the driver reached, then memory where the OOM count is above zero; else none.
    fn ended_limit(code: Option<&str>, end: Option<crate::EndOfRun>) -> Option<RunLimit> {
        match (limit_named_by(code), end) {
            (Some(RunLimit::OutputStall), Some(end)) if end.output_stall_after_exit => {
                if end.egress_ceiling_reached {
                    Some(RunLimit::EgressBytes)
                } else if end.memory_oom_kills.is_some_and(|kills| kills > 0) {
                    Some(RunLimit::Memory)
                } else {
                    None
                }
            }
            (limit, _) => limit,
        }
    }

    /// The limit the host driver named on an ended exec, by its refusal code (design 36 D6). The
    /// driver decided which bound ended the exec, from its own timer, its CPU reading, its relay's
    /// counters and the cgroup's OOM count; this repeats none of it. Every other code, a code a
    /// later driver adds among them, and no refusal are `None`.
    fn limit_named_by(code: Option<&str>) -> Option<RunLimit> {
        match code? {
            substrate_wire::SESSION_OUTPUT_BACKPRESSURE => Some(RunLimit::OutputStall),
            "exec.aperture-byte-limit" => Some(RunLimit::EgressBytes),
            "exec.memory-limit" => Some(RunLimit::Memory),
            "exec.cpu-limit" => Some(RunLimit::CpuTime),
            "exec.timeout" => Some(RunLimit::WallTime),
            _ => None,
        }
    }

    /// What the snapshot cannot serve beyond the families, in the module table's order. A free
    /// function over the probed facts, so deciding it opens no driver.
    pub(super) fn check_snapshot(
        capability: &CapabilityFacts,
        profile: &ConfinementProfile,
    ) -> Result<(), ConfinementError> {
        stdin::check_streams(profile)?;
        super::super::check_workspace_access(profile)?;
        if !profile.workspace_access.is_read_write()
            && capability.exec_workspace_scoped_write != Some(true)
        {
            return Err(DriverError::unserved(
                substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
                "The host has not proved read-only and scoped workspace mounts.",
                "workspace_access",
            )
            .into());
        }
        if !profile.writable_roots.is_empty() {
            let Some(declared) = &capability.exec_writable_roots_host else {
                return Err(ConfinementError::FamilyUnserved {
                    adapter: AdapterName::Linux,
                    family: ContractFamily::FilesystemView,
                });
            };
            if let Some(root) = profile
                .writable_roots
                .iter()
                .find(|root| !declared.contains(&root.host))
            {
                return Err(DriverError::unserved(
                    substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
                    format!("The host declared no writable root named {}.", root.host),
                    "writable_roots",
                )
                .into());
            }
        }
        if profile.workdir.is_some() && capability.exec_workdir != Some(true) {
            return Err(DriverError::unserved(
                substrate_wire::EXEC_WORKDIR_UNSERVED,
                "The host has not proved starting a run in a working directory.",
                "workdir",
            )
            .into());
        }
        if let Some(Network::Egress { apertures }) = &profile.network {
            let declared = capability.exec_egress_apertures.as_deref().unwrap_or(&[]);
            if let Some(name) = apertures
                .iter()
                .find(|name| !declared.iter().any(|aperture| &aperture.name == *name))
            {
                return Err(DriverError::refused(
                    "exec.aperture-undeclared",
                    format!("The host declared no egress aperture named {name}."),
                    "network",
                )
                .into());
            }
        }
        if !profile.secret_slots.is_empty() {
            let Some(declared) = &capability.secrets_slots else {
                return Err(DriverError::unserved(
                    "exec.secret-slots-unserved",
                    "Sealed secret slots are not served by this host.",
                    "secret_slots",
                )
                .into());
            };
            if let Some(slot) = profile
                .secret_slots
                .iter()
                .find(|slot| !declared.contains(&slot.slot))
            {
                return Err(DriverError::refused(
                    "exec.secret-slot-unknown",
                    format!("Secret slot {} is not declared on this host.", slot.slot),
                    "secret_slots",
                )
                .into());
            }
        }
        // The lexical rule `macos` applies, so a profile one adapter accepts is one the other
        // accepts (design 35 D3). What needs the view is asked at the start.
        for entry in &profile.exec_subpaths {
            crate::exec_gate::check_literal(entry)?;
        }
        for entry in &profile.exec_denied_paths {
            crate::exec_gate::check_denied_literal(entry)?;
        }
        Ok(())
    }

    #[async_trait]
    impl ConfinementAdapter for LinuxAdapter {
        fn facts(&self) -> &AdapterFacts {
            &self.facts
        }

        /// What the snapshot cannot serve beyond the families, in the module table's order.
        fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
            check_snapshot(&self.capability, profile)
        }

        fn render(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RenderedCommand, ConfinementError> {
            admitted.check_on(&self.facts, false)?;
            super::super::check_command(command)?;
            let workspace = super::super::workspace_of(admitted)?;
            let root_name = self.root_name(workspace)?;
            let input = self.exec_input(admitted, command, &root_name)?;
            let workspace = self
                .driver
                .workspace_path(&root_name)
                .map_err(ConfinementError::Driver)?;
            let (program, args) =
                self.driver
                    .processes
                    .render(&workspace, &input, &ExecLists::of(admitted))?;
            Ok(RenderedCommand { program, args })
        }

        /// Every refusal, from the contract's recheck to the driver's dispatch, writes its audit
        /// record.
        async fn start(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RunId, ConfinementError> {
            let started = self.start_run(admitted, command).await;
            admitted.audit_refusal(&self.facts, started)
        }

        async fn observe(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
            let observed = self.driver.observe_exec(run.as_str()).await?;
            let mut observation = Self::observation(run, &observed);
            if observation.state == RunState::Running {
                // Read at this moment, here and not in `wait`'s loop (design 36 D13).
                observation.process_limit_hits =
                    self.driver.running_process_limit_hits(run.as_str());
            } else {
                self.ended(run).await;
            }
            Ok(observation)
        }

        async fn output(&self, run: &RunId) -> Result<RunOutput, ConfinementError> {
            let observed = self.driver.observe_exec(run.as_str()).await?;
            // The driver records both for every run it starts; a record without one says nothing
            // of the total.
            let stdout_kept = observed
                .stdout_kept
                .unwrap_or_else(|| KeptOutput::unobserved_head(observed.stdout.len()));
            let stderr_kept = observed
                .stderr_kept
                .unwrap_or_else(|| KeptOutput::unobserved_head(observed.stderr.len()));
            Ok(RunOutput {
                stdout: observed.stdout,
                stderr: observed.stderr,
                stdout_truncated: observed.stdout_truncated,
                stderr_truncated: observed.stderr_truncated,
                stdout_kept,
                stderr_kept,
            })
        }

        async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
            loop {
                let observed = self.driver.observe_exec(run.as_str()).await?;
                let observation = Self::observation(run, &observed);
                // The driver bounds every run by its timeout, so this ends.
                if observation.state != RunState::Running && observed.output_complete {
                    self.ended(run).await;
                    return Ok(observation);
                }
                tokio::time::sleep(WAIT_POLL).await;
            }
        }

        async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
            self.driver
                .signal(
                    run.as_str(),
                    &ExecSignalInput {
                        signal,
                        grace_ms: 0,
                    },
                )
                .await?;
            Ok(())
        }

        async fn write_stdin(&self, run: &RunId, bytes: &[u8]) -> Result<(), ConfinementError> {
            self.writer(run)?.write(bytes).await
        }

        async fn close_stdin(&self, run: &RunId) -> Result<(), ConfinementError> {
            self.writer(run)?.close().await;
            Ok(())
        }

        async fn read_output(
            &self,
            run: &RunId,
            timeout: Duration,
        ) -> Result<Option<OutputEvent>, ConfinementError> {
            self.read_live(run, timeout).await
        }

        fn output_stalled(&self, run: &RunId) -> bool {
            self.readers.lock().contains_key(run) && self.driver.output_stalled(run.as_str())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use substrate_wire::WorkspaceAccess;

        /// Design 36 D6, row by row: the five codes by which the driver names a bound that ended
        /// an exec map to their limit; every other code it writes on an ended exec, a code no
        /// driver writes and no refusal map to none.
        #[test]
        fn the_refusal_code_of_an_ended_exec_names_its_limit_and_no_other_code_does() {
            for (code, limit) in [
                (
                    Some(substrate_wire::SESSION_OUTPUT_BACKPRESSURE),
                    Some(RunLimit::OutputStall),
                ),
                (
                    Some("exec.aperture-byte-limit"),
                    Some(RunLimit::EgressBytes),
                ),
                (Some("exec.memory-limit"), Some(RunLimit::Memory)),
                (Some("exec.cpu-limit"), Some(RunLimit::CpuTime)),
                (Some("exec.timeout"), Some(RunLimit::WallTime)),
                (Some("exec.metrics-unavailable"), None),
                (Some(substrate_wire::SESSION_OUTPUT_LIMIT), None),
                (Some(substrate_wire::WORKDIR_REFUSALS[2].code), None),
                (Some("exec.some-later-bound"), None),
                (None, None),
            ] {
                assert_eq!(limit_named_by(code), limit, "{code:?}");
            }
        }

        /// The end the driver read: the OOM count, the stall's mark and the ceiling.
        fn end(after_exit: bool, oom_kills: u64, egress: bool) -> crate::EndOfRun {
            crate::EndOfRun {
                memory_oom_kills: Some(oom_kills),
                process_limit_hits: Some(0),
                output_stall_after_exit: after_exit,
                egress_ceiling_reached: egress,
            }
        }

        const STALL: Option<&str> = Some(substrate_wire::SESSION_OUTPUT_BACKPRESSURE);

        /// Design 36 D5: the stall the driver records as `session.output-backpressure` names
        /// `OutputStall` where it came before the child's exit, also beside an OOM kill (D4), and
        /// names no limit where it came after the program's own exit. A stall after the exit
        /// always leaves that code on the record, so no other code comes with the mark.
        #[test]
        fn a_stall_after_the_childs_exit_names_no_limit_and_one_before_names_the_stall() {
            assert_eq!(
                ended_limit(STALL, Some(end(false, 0, false))),
                Some(RunLimit::OutputStall)
            );
            assert_eq!(ended_limit(STALL, None), Some(RunLimit::OutputStall));
            assert_eq!(ended_limit(STALL, Some(end(true, 0, false))), None);
            assert_eq!(
                ended_limit(STALL, Some(end(false, 1, false))),
                Some(RunLimit::OutputStall)
            );
        }

        /// Design 36 D1, D4, D5: an exec the OOM kill ended, whose unread output then stalled
        /// after its exit. The driver's record carries `session.output-backpressure`
        /// (`record_pipe_backpressure` replaces any refusal, and `record_resource_bound` then
        /// writes none), its end the OOM count and the stall's mark. The stall ended nothing; the
        /// OOM kill ended the run, so `Memory` is named.
        #[test]
        fn an_exec_the_oom_kill_ended_whose_output_stalled_after_its_exit_names_memory() {
            assert_eq!(
                ended_limit(STALL, Some(end(true, 1, false))),
                Some(RunLimit::Memory),
                "an exec the OOM kill ended, whose output stalled after its exit, names no memory \
                 limit"
            );
        }

        /// Design 36 D4, D5: an exec an aperture's byte ceiling ended, whose unread output then
        /// stalled after its exit, names `EgressBytes`; with an OOM kill as well, egress bytes
        /// come first. The record carries `session.output-backpressure` in both.
        #[test]
        fn an_exec_the_aperture_ceiling_ended_whose_output_stalled_after_its_exit_names_egress_bytes()
         {
            assert_eq!(
                ended_limit(STALL, Some(end(true, 0, true))),
                Some(RunLimit::EgressBytes),
                "an exec the aperture ceiling ended names no egress limit"
            );
            assert_eq!(
                ended_limit(STALL, Some(end(true, 1, true))),
                Some(RunLimit::EgressBytes),
                "egress bytes are not named before memory"
            );
        }

        fn scoped() -> ConfinementProfile {
            ConfinementProfile {
                workspace_access: WorkspaceAccess::Scoped {
                    writable_subtrees: vec!["out".to_owned()],
                },
                ..ConfinementProfile::default()
            }
        }

        // A5: a snapshot that did not prove `exec.workspace-scoped-write` refuses a narrower
        // workspace by the host's own name, before any start; no driver opens.
        #[test]
        fn a_snapshot_without_scoped_write_refuses_a_narrower_workspace() {
            for access in [
                WorkspaceAccess::ReadOnly,
                WorkspaceAccess::Scoped {
                    writable_subtrees: vec!["out".to_owned()],
                },
            ] {
                for proved in [None, Some(false)] {
                    let capability = CapabilityFacts {
                        exec_workspace_scoped_write: proved,
                        ..CapabilityFacts::default()
                    };
                    let profile = ConfinementProfile {
                        workspace_access: access.clone(),
                        ..ConfinementProfile::default()
                    };
                    let error = check_snapshot(&capability, &profile)
                        .expect_err("a narrower workspace is refused");
                    assert_eq!(
                        error.code(),
                        substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
                        "{access:?} {proved:?}: {error}"
                    );
                    assert_eq!(error.status(), 501);
                }
            }
        }

        #[test]
        fn a_snapshot_with_scoped_write_admits_it_and_checks_the_shared_rules_first() {
            let capability = CapabilityFacts {
                exec_workspace_scoped_write: Some(true),
                ..CapabilityFacts::default()
            };
            check_snapshot(&capability, &scoped()).expect("served");
            check_snapshot(&CapabilityFacts::default(), &ConfinementProfile::default())
                .expect("the whole workspace needs no fact");
            let git = ConfinementProfile {
                workspace_access: WorkspaceAccess::Scoped {
                    writable_subtrees: vec![".git".to_owned()],
                },
                ..ConfinementProfile::default()
            };
            assert_eq!(
                check_snapshot(&CapabilityFacts::default(), &git)
                    .expect_err("refused")
                    .code(),
                crate::confinement::WRITABLE_SUBTREE_GIT
            );
        }

        /// A snapshot with the confinement floor and the workspace family.
        fn floor() -> CapabilitySnapshot {
            CapabilitySnapshot {
                snapshot: format!("sha256:{}", "0".repeat(64)),
                driver: substrate_wire::HostDriverKind::Host,
                driver_version: "fixture".to_owned(),
                config_generation: 1,
                probed_at: chrono::Utc::now(),
                valid_until: None,
                facts: CapabilityFacts {
                    workspace_guarded_io: Some(true),
                    exec_argv_only: Some(true),
                    exec_namespaces: Some(substrate_wire::NamespaceFacts {
                        user: true,
                        mount: true,
                        pid: true,
                        ipc: true,
                        uts: true,
                        network: true,
                    }),
                    ..CapabilityFacts::default()
                },
                withheld: Vec::new(),
            }
        }

        fn listing(entries: &[&str]) -> ConfinementProfile {
            ConfinementProfile {
                workspace: Some(std::path::PathBuf::from("/srv/workspaces/ws_1")),
                exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
                ..ConfinementProfile::default()
            }
        }

        /// Design 35 D14: `exec_subpaths` is proved where the driver's probe held, on the floor
        /// the helper runs in, and nowhere else. The snapshot alone proves none of it.
        #[test]
        fn exec_subpaths_is_proved_by_the_probe_on_the_floor_and_by_nothing_else() {
            let held = ExecListProof {
                subpaths: true,
                ..ExecListProof::default()
            };
            assert!(proof(&floor(), &held).proves(&Fact::ExecSubpaths));
            assert!(!proof(&floor(), &ExecListProof::default()).proves(&Fact::ExecSubpaths));
            let mut without_floor = floor();
            without_floor.facts.exec_namespaces = None;
            assert!(!proof(&without_floor, &held).proves(&Fact::ExecSubpaths));
        }

        /// Design 35 D27: what the open withheld is in the proof, with its cause, and a fact an
        /// entry names is never proved. Serialized, a proof with no entry has no `withheld`
        /// member, so every proof serialized before is unchanged.
        #[test]
        fn a_withheld_fact_is_in_the_proof_and_is_never_proved() {
            let entry = |file_system: Option<&str>| crate::confinement::FactWithheld {
                fact: Fact::ExecSubpaths,
                cause: if file_system.is_some() {
                    "file-system-unlisted"
                } else {
                    "mount-id-unreported"
                },
                address: "config.workspace_root",
                path: "/data/workspaces".to_owned(),
                file_system: file_system.map(str::to_owned),
            };
            let entries = vec![entry(Some("xfs")), entry(None)];
            let facts = proof(
                &floor(),
                &ExecListProof {
                    subpaths: false,
                    denied_paths: false,
                    withheld: entries.clone(),
                },
            );
            assert!(!facts.proves(&Fact::ExecSubpaths));
            assert_eq!(facts.withheld, entries);
            // Whatever the probe's member says.
            let contradictory = proof(
                &floor(),
                &ExecListProof {
                    subpaths: true,
                    denied_paths: true,
                    withheld: entries.clone(),
                },
            );
            assert!(!contradictory.proves(&Fact::ExecSubpaths));
            assert!(!contradictory.proves(&Fact::ExecDeniedPaths));
            assert_eq!(contradictory.withheld, entries);

            let serialized = serde_json::to_value(&facts).expect("the proof serializes");
            assert_eq!(
                serialized["withheld"],
                serde_json::json!([
                    {
                        "fact": "exec_subpaths",
                        "cause": "file-system-unlisted",
                        "address": "config.workspace_root",
                        "path": "/data/workspaces",
                        "file_system": "xfs",
                    },
                    {
                        "fact": "exec_subpaths",
                        "cause": "mount-id-unreported",
                        "address": "config.workspace_root",
                        "path": "/data/workspaces",
                    },
                ])
            );
            for exec_lists in [
                ExecListProof::default(),
                ExecListProof {
                    subpaths: true,
                    ..ExecListProof::default()
                },
            ] {
                let plain = proof(&floor(), &exec_lists);
                assert!(plain.withheld.is_empty());
                let serialized = serde_json::to_value(&plain).expect("the proof serializes");
                assert!(serialized.get("withheld").is_none(), "{serialized}");
                // And the driver's own record, which the snapshot identity is computed over.
                let own = serde_json::to_value(&exec_lists).expect("the record serializes");
                assert!(own.get("withheld").is_none(), "{own}");
            }
        }

        /// Design 35 D22: the lists the adapter hands the driver carry every entry of the
        /// admitted profile, so no admitted profile with a list starts as one without.
        #[test]
        fn the_lists_of_an_admitted_profile_carry_every_entry() {
            let facts = proof(
                &floor(),
                &ExecListProof {
                    subpaths: true,
                    ..ExecListProof::default()
                },
            );
            let check =
                |profile: &ConfinementProfile| check_snapshot(&CapabilityFacts::default(), profile);
            for entries in [
                vec!["bin"],
                vec!["bin", "/usr/bin", "target/debug"],
                vec!["/usr/bin/git"],
            ] {
                let admitted = crate::confinement::admission(&facts, &listing(&entries), check)
                    .expect("a listed profile on a host that proved the list");
                let lists = ExecLists::of(&admitted);
                assert!(!lists.is_empty(), "{entries:?}");
                assert_eq!(lists.subpaths(), entries, "every entry, in order");
            }
            let admitted = crate::confinement::admission(&facts, &listing(&[]), check)
                .expect("a profile without a list");
            assert!(ExecLists::of(&admitted).is_empty());
            // Both lists (design 35 D10): every deny entry is carried beside the allow entries.
            let both = proof(
                &floor(),
                &ExecListProof {
                    subpaths: true,
                    denied_paths: true,
                    ..ExecListProof::default()
                },
            );
            let denied = vec!["bin/tool".to_owned(), "/usr/bin/id".to_owned()];
            let profile = ConfinementProfile {
                exec_denied_paths: denied.clone(),
                ..listing(&["bin", "/usr/bin"])
            };
            let admitted = crate::confinement::admission(&both, &profile, check)
                .expect("both lists on a host that proved both");
            let lists = ExecLists::of(&admitted);
            assert!(!lists.is_empty());
            assert_eq!(lists.subpaths(), ["bin", "/usr/bin"]);
            assert_eq!(lists.denied(), denied, "every deny entry, in order");
            // Without the fact there is no admitted profile to take lists of.
            let unproved = proof(&floor(), &ExecListProof::default());
            let error = crate::confinement::admission(&unproved, &listing(&["bin"]), check)
                .expect_err("a list on a host that did not prove it");
            assert!(
                matches!(
                    &error,
                    ConfinementError::FactUnserved { fact, .. } if *fact == Fact::ExecSubpaths
                ),
                "{error}"
            );
            assert_eq!(error.status(), 501);
        }

        /// Design 35 D3: the lexical rule is `macos`'s and is admission's. An entry no adapter
        /// can carry is refused before any start.
        #[test]
        fn an_entry_no_adapter_can_carry_is_refused_at_admission() {
            for entry in [
                "bin/../other",
                "/usr/./bin",
                "a//b",
                "it\"s",
                "",
                "back\\slash",
            ] {
                let error = check_snapshot(&CapabilityFacts::default(), &listing(&[entry]))
                    .expect_err(entry);
                assert_eq!(
                    error.code(),
                    crate::seatbelt::PATH_UNREPRESENTABLE,
                    "{entry:?}: {error}"
                );
                assert_eq!(error.status(), 422, "{entry:?}");
            }
            check_snapshot(
                &CapabilityFacts::default(),
                &listing(&["bin", "target/debug", "/usr/bin", "/opt/t/bin/tool"]),
            )
            .expect("entries every adapter can carry; what needs the view is the start's");
        }

        /// The same rule for an entry of the deny-list (design 35 D10: "an entry has the form
        /// of an `exec_subpaths` entry"), at admission and with its own address. An entry
        /// that does not exist is no fault of its form.
        #[test]
        fn a_deny_entry_no_adapter_can_carry_is_refused_at_admission() {
            let denying = |denied: &[&str]| ConfinementProfile {
                exec_denied_paths: denied.iter().map(|entry| (*entry).to_owned()).collect(),
                ..listing(&["bin", "/usr/bin"])
            };
            for entry in [
                "bin/../other",
                "/usr/./bin",
                "a//b",
                "it\"s",
                "",
                "back\\slash",
            ] {
                let error = check_snapshot(&CapabilityFacts::default(), &denying(&[entry]))
                    .expect_err(entry);
                assert_eq!(
                    error.code(),
                    crate::seatbelt::PATH_UNREPRESENTABLE,
                    "{entry:?}: {error}"
                );
                assert_eq!(error.status(), 422, "{entry:?}");
                let ConfinementError::Driver(driver) = &error else {
                    panic!("{entry:?}: the driver's refusal: {error}");
                };
                assert_eq!(
                    driver.address.as_deref(),
                    Some("exec_denied_paths"),
                    "{entry:?}"
                );
            }
            check_snapshot(
                &CapabilityFacts::default(),
                &denying(&["bin/tool", "/usr/bin/id", "later/absent", "/opt/t"]),
            )
            .expect("deny entries every adapter can carry; what needs the view is the start's");
        }

        /// Design 35 D14, D15: **`exec_denied_paths` is proved by its own probe, beside the
        /// allow-list's, and withheld by its own entry.** A host whose second probe failed
        /// proves `exec_subpaths` alone. A record that names only the deny-list as withheld
        /// leaves the allow-list proved and the deny-list not, whatever its probe says.
        #[test]
        fn exec_denied_paths_is_proved_by_its_own_probe_and_withheld_by_its_own_entry() {
            let both = ExecListProof {
                subpaths: true,
                denied_paths: true,
                ..ExecListProof::default()
            };
            let proved = proof(&floor(), &both);
            assert!(proved.proves(&Fact::ExecSubpaths));
            assert!(proved.proves(&Fact::ExecDeniedPaths));

            let first_only = proof(
                &floor(),
                &ExecListProof {
                    denied_paths: false,
                    ..both.clone()
                },
            );
            assert!(first_only.proves(&Fact::ExecSubpaths));
            assert!(
                !first_only.proves(&Fact::ExecDeniedPaths),
                "the second probe failed, and the fact is proved by nothing else"
            );
            let second_only = proof(
                &floor(),
                &ExecListProof {
                    subpaths: false,
                    ..both.clone()
                },
            );
            assert!(!second_only.proves(&Fact::ExecDeniedPaths));
            let mut without_floor = floor();
            without_floor.facts.exec_namespaces = None;
            assert!(!proof(&without_floor, &both).proves(&Fact::ExecDeniedPaths));

            let entries = vec![crate::confinement::FactWithheld {
                fact: Fact::ExecDeniedPaths,
                cause: "mount-id-unreported",
                address: "config.workspace_root",
                path: "/data/workspaces".to_owned(),
                file_system: None,
            }];
            let own = proof(
                &floor(),
                &ExecListProof {
                    withheld: entries.clone(),
                    ..both
                },
            );
            assert!(own.proves(&Fact::ExecSubpaths));
            assert!(
                !own.proves(&Fact::ExecDeniedPaths),
                "a fact an entry names is never proved"
            );
            assert_eq!(own.withheld, entries);
        }

        /// Design 36 D12 on `linux` in the gate's test step: a count of refused process
        /// creations above zero names no limit and changes none, for every code the driver
        /// writes on an ended exec, with and without a stall after the exit.
        #[test]
        fn a_count_of_refused_creations_names_no_limit_on_linux() {
            let with = |hits: Option<u64>, after_exit: bool, oom: u64, egress: bool| {
                let mut end = end(after_exit, oom, egress);
                end.process_limit_hits = hits;
                end
            };
            for code in [
                Some(substrate_wire::SESSION_OUTPUT_BACKPRESSURE),
                Some("exec.aperture-byte-limit"),
                Some("exec.memory-limit"),
                Some("exec.cpu-limit"),
                Some("exec.timeout"),
                Some("exec.metrics-unavailable"),
                Some(substrate_wire::SESSION_OUTPUT_LIMIT),
                None,
            ] {
                for (after_exit, oom, egress) in [
                    (false, 0, false),
                    (true, 0, false),
                    (true, 1, false),
                    (true, 0, true),
                ] {
                    let counted = ended_limit(code, Some(with(Some(9), after_exit, oom, egress)));
                    for hits in [None, Some(0)] {
                        assert_eq!(
                            counted,
                            ended_limit(code, Some(with(hits, after_exit, oom, egress))),
                            "{code:?} {after_exit} {oom} {egress}: a count of 9 changed the limit"
                        );
                    }
                }
            }
            assert_eq!(
                ended_limit(None, Some(with(Some(9), false, 0, false))),
                None
            );
        }

        /// An exec observation in `state`, its end read as `end_of_run`, with `refusal`.
        fn exec_observation(
            state: ExecState,
            refusal: Option<&str>,
            end_of_run: Option<crate::EndOfRun>,
        ) -> crate::ExecObservation {
            crate::ExecObservation {
                resource: substrate_wire::Exec {
                    unrecorded_output: None,
                    id: "ex_count".to_owned(),
                    kind: substrate_wire::ExecKind::Exec,
                    workspace: "ws_test".to_owned(),
                    state,
                    observed_at: chrono::Utc::now(),
                    requested: ConfinementRequest {
                        requires: None,
                        capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                        network: NetworkMode::None,
                        aperture: None,
                        apertures: Vec::new(),
                        profile: SandboxProfile::Workspace,
                        required: true,
                        exec_subpaths: None,
                        exec_denied_paths: Vec::new(),
                    },
                    applied: None,
                    exit: (state != ExecState::Running).then_some(substrate_wire::ExecExit {
                        code: None,
                        signal: Some(Signal::Kill),
                    }),
                    usage: None,
                    lease: None,
                    refusal: refusal.map(|code| substrate_wire::ExecRefusal {
                        class: substrate_wire::ErrorClass::Exhausted,
                        code: code.to_owned(),
                        message: "bound".to_owned(),
                    }),
                },
                stdout: Vec::new(),
                stderr: Vec::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                stdout_kept: None,
                stderr_kept: None,
                output_complete: state != ExecState::Running,
                cgroup: Some("substrate-ex_count".to_owned()),
                leader_pid: None,
                end_of_run,
            }
        }

        /// Design 36 D9, D12, D14 on `linux` in the gate's test step: the ended observation
        /// (which `wait`, `observe` and `run_ended` all take) carries the count the driver read
        /// at the end, unchanged; the wall-time bound that ended the run is the limit it names;
        /// an end without a count, or no end read, is absent, never zero; and the observation
        /// function itself reads nothing for a running exec.
        #[test]
        fn the_ended_linux_observation_carries_the_drivers_end_of_run_count() {
            let run = RunId::new("ex_count".to_owned());
            let mut read = end(false, 0, false);
            read.process_limit_hits = Some(7);
            let ended = LinuxAdapter::observation(
                &run,
                &exec_observation(ExecState::Cancelled, Some("exec.timeout"), Some(read)),
            );
            assert_eq!(ended.state, RunState::Ended, "{ended:?}");
            assert_eq!(ended.limit, Some(RunLimit::WallTime), "{ended:?}");
            assert_eq!(ended.process_limit_hits, Some(7), "{ended:?}");
            read.process_limit_hits = None;
            let unread = LinuxAdapter::observation(
                &run,
                &exec_observation(ExecState::Exited, None, Some(read)),
            );
            assert_eq!(unread.process_limit_hits, None, "{unread:?}");
            let rebuilt =
                LinuxAdapter::observation(&run, &exec_observation(ExecState::Exited, None, None));
            assert_eq!(rebuilt.process_limit_hits, None, "{rebuilt:?}");
            let running =
                LinuxAdapter::observation(&run, &exec_observation(ExecState::Running, None, None));
            assert_eq!(running.state, RunState::Running, "{running:?}");
            assert_eq!(running.process_limit_hits, None, "{running:?}");
        }
    }
}
