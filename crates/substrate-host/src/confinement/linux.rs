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
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use substrate_wire::{
        CapabilitySnapshot, ConfinementRequest, ExecEnvironment, ExecLimits, ExecSignalInput,
        ExecStartInput, ExecState, NetworkMode, SandboxProfile, Signal, WorkspaceAccess,
        WritableRoot,
    };

    use super::super::{
        AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
        ContractFamily, Fact, Network, ProcessBounds, RenderedCommand, RunExit, RunId,
        RunObservation, RunOutput, RunState,
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
            snapshot: machine.snapshot,
            driver,
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
        snapshot: String,
        driver: Arc<HostDriver>,
    }

    impl LinuxAdapter {
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
                workspace_access: WorkspaceAccess::ReadWrite,
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
            }
        }
    }

    #[async_trait]
    impl ConfinementAdapter for LinuxAdapter {
        fn facts(&self) -> &AdapterFacts {
            &self.facts
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

        async fn start(
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
            match self.driver.start_exec(&id, &root_name, &input).await {
                DispatchOutcome::Observed(_) => {
                    let run = RunId::new(id);
                    admitted.audit_start(&run, &command.argv[0]);
                    Ok(run)
                }
                DispatchOutcome::NotDispatched(error)
                | DispatchOutcome::ContainedAbsent(error)
                | DispatchOutcome::OutcomeUnknown(error) => Err(ConfinementError::Driver(error)),
            }
        }

        async fn observe(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
            let observed = self.driver.observe_exec(run.as_str()).await?;
            Ok(Self::observation(run, &observed))
        }

        async fn output(&self, run: &RunId) -> Result<RunOutput, ConfinementError> {
            let observed = self.driver.observe_exec(run.as_str()).await?;
            Ok(RunOutput {
                stdout: observed.stdout,
                stderr: observed.stderr,
                stdout_truncated: observed.stdout_truncated,
                stderr_truncated: observed.stderr_truncated,
            })
        }

        async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
            loop {
                let observed = self.driver.observe_exec(run.as_str()).await?;
                let observation = Self::observation(run, &observed);
                // The driver bounds every run by its timeout, so this ends.
                if observation.state != RunState::Running && observed.output_complete {
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
    }
}
