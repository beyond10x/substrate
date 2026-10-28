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
//!
//! `network_ingress` is served by no adapter. `paths_are_host_paths`, `sealed_paths`,
//! `exec_subpaths`, `private_tmpdir` and `host_network` are not proved, and a profile requiring
//! one is refused `confinement.fact-unserved`.
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

    use super::super::stdin::{self, StdinPipe, StdinWriter};
    use super::super::{
        AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
        ConfinementProfile, ContractFamily, Fact, KeptOutput, Network, ProcessBounds,
        RenderedCommand, RunExit, RunId, RunObservation, RunOutput, RunState, StdinMode,
    };
    use super::LinuxRequest;
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

    pub(super) fn open(
        request: LinuxRequest,
    ) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
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
            facts: proof(&machine),
            capability: machine.facts,
            snapshot: machine.snapshot,
            driver,
            writers: parking_lot::Mutex::new(HashMap::new()),
        }))
    }

    /// The families and facts a host driver's capability snapshot proves.
    pub fn proof(machine: &CapabilitySnapshot) -> AdapterFacts {
        let facts = &machine.facts;
        let floor = facts.exec_argv_only == Some(true) && facts.exec_namespaces.is_some();
        let limits = facts
            .exec_cgroup_limits
            .as_ref()
            .is_some_and(|limits| limits.processes && limits.memory && limits.cpu);
        AdapterFacts::new(
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
                (
                    Fact::IpcSignalsContained,
                    floor
                        && facts
                            .exec_namespaces
                            .as_ref()
                            .is_some_and(|namespaces| namespaces.pid),
                ),
            ],
        )
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
                    capability_snapshot: self.snapshot.clone(),
                    network,
                    aperture,
                    apertures,
                    profile: SandboxProfile::Workspace,
                    required: true,
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
            self.forget_writers();
            match self
                .driver
                .start_exec_for_adapter(&id, &root_name, &input, retention, piped.is_piped())
                .await
            {
                DispatchOutcome::Observed(_) => {
                    let run = RunId::new(id.clone());
                    if let StdinMode::Piped { limit_bytes } = piped {
                        let writer = StdinWriter::new(
                            limit_bytes,
                            Box::new(HostPipe {
                                driver: Arc::clone(&self.driver),
                                id: id.clone(),
                            }),
                        );
                        self.writers.lock().insert(run.clone(), Arc::clone(&writer));
                        let driver = Arc::clone(&self.driver);
                        tokio::spawn(async move {
                            driver.processes.wait_ended(&id).await;
                            writer.end().await;
                        });
                    }
                    admitted.audit_start(&run, &command.argv[0]);
                    Ok(run)
                }
                DispatchOutcome::NotDispatched(error)
                | DispatchOutcome::ContainedAbsent(error)
                | DispatchOutcome::OutcomeUnknown(error) => Err(ConfinementError::Driver(error)),
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
                memory_oom_kills: None,
            }
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
            let (program, args) = self.driver.processes.render(&workspace, &input)?;
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
            let observation = Self::observation(run, &observed);
            if observation.state != RunState::Running {
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
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use substrate_wire::WorkspaceAccess;

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
    }
}
