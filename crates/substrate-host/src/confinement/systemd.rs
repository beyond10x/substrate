//! The `systemd` adapter: a delegated user scope as the bounds provider around `local` or `linux`
//! (ADR 0109 decision 3).
//!
//! Opening it asks the user's systemd manager, over its session bus, for one transient scope with
//! `Delegate=yes` and moves **this process** into it — what a caller-side
//! `systemd-run --user --scope -p Delegate=yes` around the embedder did before, done once here.
//! The process sits in the scope's `embedder` leaf, so the scope's root stays process-free and
//! carries `cpu`, `memory` and `pids` for its children. Every `systemd` adapter the process opens
//! shares that scope; the process stays in it after the adapter is dropped, as it stayed inside a
//! caller's `systemd-run` scope.
//!
//! | wraps | what the scope gives it |
//! |---|---|
//! | `local` | one child cgroup per run: `pids.max`, `memory.max` with `memory.swap.max` 0, `memory.oom.group`, the timeout and cumulative CPU budget enforced from `cpu.stat`, and `cgroup.kill`; the run joins its cgroup before it executes, and nothing of its tree outlives its leader |
//! | `linux` | the scope as the host driver's delegated `cgroup_root`, so `linux` proves `limits` and `whole_tree_kill` itself |
//!
//! Around `local` it serves `local`'s families plus `limits`, and proves `local`'s facts plus
//! `whole_tree_kill`; a run is still declared insecure and needs a reason. Around `linux` it
//! serves and proves exactly what `linux` proved in the scope. One scope is the delegated root of
//! one `linux` driver at a time: a second `systemd` around `linux` while one is open is refused.
//!
//! Around `local` the bounds hold the tree a run makes, not a run that works against them: the run
//! has this process's uid, and the cgroup files beneath a user scope are that uid's to write, so a
//! run can raise its own `pids.max` or leave its cgroup. That is why the run stays declared
//! insecure; a run that must not lift its bounds needs `linux`.
//!
//! The open is refused `confinement.adapter-unserved`, before anything moves, when no user manager
//! is reachable: `DBUS_SESSION_BUS_ADDRESS` unset or empty, no `systemctl` or `busctl`, or
//! `systemctl --user` not answering. It is refused the same when the scope the manager made does
//! not carry `cpu`, `memory` and `pids` or `cgroup.kill`, or when `linux` did not prove `limits`
//! and `whole_tree_kill` in it: a `systemd` adapter that cannot bound a run is not opened
//! (invariant 3).

use std::sync::Arc;

use super::linux::LinuxRequest;
use super::local::LocalRequest;
use super::{ConfinementAdapter, ConfinementError, InnerAdapter};

/// The variable naming the user's session bus, through which the user manager is reached.
pub const SESSION_BUS_ENV: &str = "DBUS_SESSION_BUS_ADDRESS";

/// Each output stream of a run around `local` is kept up to this many bytes and drained past it;
/// a profile's `io_bounds.output_bytes` may lower it, not raise it.
pub const SYSTEMD_OUTPUT_LIMIT_BYTES: usize = super::local::LOCAL_OUTPUT_LIMIT_BYTES;

/// Runs one adapter around `local` tracks; past it, ended runs are forgotten oldest first, and a
/// start with every tracked run still running is refused `exec.tracking-capacity`.
pub const SYSTEMD_MAX_TRACKED_RUNS: usize = 128;

/// The adapter `systemd` wraps, as it would be opened on its own.
#[derive(Debug, Clone)]
pub enum InnerRequest {
    Local(LocalRequest),
    Linux(Box<LinuxRequest>),
}

impl InnerRequest {
    pub const fn inner(&self) -> InnerAdapter {
        match self {
            Self::Local(_) => InnerAdapter::Local,
            Self::Linux(_) => InnerAdapter::Linux,
        }
    }
}

/// What opening `systemd` takes: the adapter it wraps, and the session bus its user manager is
/// reached through. `None`, or an empty address, is a host without one.
#[derive(Debug, Clone)]
pub struct SystemdRequest {
    pub inner: InnerRequest,
    pub session_bus: Option<std::ffi::OsString>,
}

impl SystemdRequest {
    /// `inner`, reached through this process's [`SESSION_BUS_ENV`].
    pub fn new(inner: InnerRequest) -> Self {
        Self {
            inner,
            session_bus: std::env::var_os(SESSION_BUS_ENV),
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn open(
    _request: SystemdRequest,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    super::unserved(
        super::AdapterName::Systemd,
        "the systemd adapter needs Linux",
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn open(
    request: SystemdRequest,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    imp::open(request)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::OsString;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use parking_lot::Mutex;
    use substrate_wire::Signal;
    use tokio::io::{AsyncRead, AsyncReadExt as _};
    use tokio::sync::watch;

    use super::super::{
        AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
        ConfinementProfile, ContractFamily, FACT_ORDER, FAMILY_ORDER, Fact, InnerAdapter,
        ProcessBounds, RenderedCommand, RunExit, RunId, RunObservation, RunOutput, RunState,
    };
    use super::{
        InnerRequest, SESSION_BUS_ENV, SYSTEMD_MAX_TRACKED_RUNS, SYSTEMD_OUTPUT_LIMIT_BYTES,
        SystemdRequest,
    };
    use crate::DriverError;

    const CGROUP_FS: &str = "/sys/fs/cgroup";
    /// The leaf of the scope this process moves into, so the scope's root stays process-free.
    const EMBEDDER: &str = "embedder";
    /// How long the user manager has to move this process into a new scope.
    const SCOPE_START: Duration = Duration::from_secs(10);
    /// How long a run's killed tree has to leave its cgroup before the run is reported unknown.
    const TREE_DRAIN: Duration = Duration::from_secs(10);
    const POLL: Duration = Duration::from_millis(10);
    /// How often a run's timeout and CPU budget are read.
    const WATCH: Duration = Duration::from_millis(20);
    const CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];
    const TOOL_DIRECTORIES: [&str; 2] = ["/usr/bin", "/bin"];

    /// The bounds a run around `local` gets when its profile states none: the `linux` adapter's.
    pub const DEFAULT_BOUNDS: ProcessBounds = ProcessBounds {
        timeout_ms: 600_000,
        processes: 256,
        memory_bytes: 1 << 30,
        cpu_millis: 600_000,
    };

    /// The scope this process sits in, once a `systemd` adapter moved it there.
    static SCOPE: Mutex<Option<PathBuf>> = Mutex::new(None);
    /// Whether an open `systemd` around `linux` holds the scope as its driver's root.
    static LINUX_HELD: AtomicBool = AtomicBool::new(false);

    fn unserved(detail: impl Into<String>) -> ConfinementError {
        ConfinementError::AdapterUnserved {
            adapter: AdapterName::Systemd,
            detail: detail.into(),
        }
    }

    fn tool(name: &str) -> Option<PathBuf> {
        TOOL_DIRECTORIES
            .iter()
            .map(|directory| Path::new(directory).join(name))
            .find(|path| path.is_file())
    }

    /// The user manager, reached.
    struct Manager {
        bus: OsString,
        busctl: PathBuf,
    }

    impl Manager {
        fn reach(bus: Option<OsString>) -> Result<Self, ConfinementError> {
            let bus = bus.filter(|bus| !bus.is_empty()).ok_or_else(|| {
                unserved(format!(
                    "{SESSION_BUS_ENV} is unset: no user systemd manager is reachable"
                ))
            })?;
            let systemctl = tool("systemctl").ok_or_else(|| unserved("no systemctl"))?;
            let busctl = tool("busctl").ok_or_else(|| unserved("no busctl"))?;
            let manager = Self { bus, busctl };
            manager.call(
                &systemctl,
                &["--user", "show", "--property=Version", "--value"],
            )?;
            Ok(manager)
        }

        /// Runs `program` with only the session bus in its environment.
        fn call(&self, program: &Path, args: &[&str]) -> Result<String, ConfinementError> {
            let output = std::process::Command::new(program)
                .args(args)
                .env_clear()
                .env(SESSION_BUS_ENV, &self.bus)
                .stdin(Stdio::null())
                .output()
                .map_err(|error| unserved(format!("{}: {error}", program.display())))?;
            if !output.status.success() {
                return Err(unserved(format!(
                    "{} {}: {}",
                    program.display(),
                    args.first().copied().unwrap_or_default(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }

        /// Asks for a transient scope with `Delegate=yes` holding this process, and returns its
        /// cgroup once this process is in it, in its `embedder` leaf, with the controllers enabled
        /// for the scope's children.
        fn create_scope(&self) -> Result<PathBuf, ConfinementError> {
            let unit = format!(
                "substrate-confinement-{}.scope",
                ulid::Ulid::generate().to_string().to_ascii_lowercase()
            );
            let pid = std::process::id().to_string();
            self.call(
                &self.busctl,
                &[
                    "--user",
                    "call",
                    "org.freedesktop.systemd1",
                    "/org/freedesktop/systemd1",
                    "org.freedesktop.systemd1.Manager",
                    "StartTransientUnit",
                    "ssa(sv)a(sa(sv))",
                    &unit,
                    "fail",
                    "4",
                    "PIDs",
                    "au",
                    "1",
                    &pid,
                    "Delegate",
                    "b",
                    "true",
                    // The scope bounds nothing itself: each run's own cgroup carries its bounds.
                    "TasksMax",
                    "t",
                    "18446744073709551615",
                    "CollectMode",
                    "s",
                    "inactive-or-failed",
                    "0",
                ],
            )?;
            let started = Instant::now();
            let scope = loop {
                if let Some(current) = own_cgroup()
                    && current.file_name().and_then(|name| name.to_str()) == Some(unit.as_str())
                {
                    break current;
                }
                if started.elapsed() > SCOPE_START {
                    return Err(unserved(format!(
                        "the user manager did not move this process into {unit}"
                    )));
                }
                std::thread::sleep(POLL);
            };
            let controllers =
                std::fs::read_to_string(scope.join("cgroup.controllers")).unwrap_or_default();
            if let Some(missing) = CONTROLLERS
                .iter()
                .find(|wanted| !controllers.split_whitespace().any(|have| have == **wanted))
            {
                return Err(unserved(format!(
                    "the user manager delegates no {missing} controller to {unit} (has: {})",
                    controllers.trim()
                )));
            }
            if !scope.join("cgroup.kill").is_file() {
                return Err(unserved(format!("{unit} has no cgroup.kill")));
            }
            let embedder = scope.join(EMBEDDER);
            match std::fs::create_dir(&embedder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(unserved(format!("{}: {error}", embedder.display()))),
            }
            std::fs::write(embedder.join("cgroup.procs"), &pid).map_err(|error| {
                unserved(format!("moving into {}: {error}", embedder.display()))
            })?;
            std::fs::write(scope.join("cgroup.subtree_control"), "+cpu +memory +pids").map_err(
                |error| unserved(format!("enabling controllers beneath {unit}: {error}")),
            )?;
            Ok(scope)
        }
    }

    /// This process's cgroup v2 directory.
    fn own_cgroup() -> Option<PathBuf> {
        let cgroups = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let path = cgroups.lines().find_map(|line| line.strip_prefix("0::"))?;
        Some(Path::new(CGROUP_FS).join(path.trim_start_matches('/')))
    }

    /// The scope this process sits in, made now if no `systemd` adapter made it yet or the process
    /// has since been moved out of it.
    fn process_scope(manager: &Manager) -> Result<PathBuf, ConfinementError> {
        let mut held = SCOPE.lock();
        if let Some(scope) = held.as_ref()
            && own_cgroup().as_deref() == Some(scope.join(EMBEDDER).as_path())
        {
            return Ok(scope.clone());
        }
        let scope = manager.create_scope()?;
        *held = Some(scope.clone());
        Ok(scope)
    }

    pub(super) fn open(
        request: SystemdRequest,
    ) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
        let manager = Manager::reach(request.session_bus)?;
        match request.inner {
            InnerRequest::Local(local) => open_local(&manager, local),
            InnerRequest::Linux(linux) => open_linux(&manager, *linux),
        }
    }

    /// The facts `systemd` around `inner` proves: `inner`'s, plus what the scope adds.
    fn proof(
        inner: InnerAdapter,
        proved: &AdapterFacts,
        families: &[ContractFamily],
        facts: &[Fact],
    ) -> AdapterFacts {
        AdapterFacts::new(
            AdapterName::Systemd,
            Some(inner),
            &FAMILY_ORDER
                .iter()
                .map(|family| {
                    (
                        family.clone(),
                        proved.serves(family) || families.contains(family),
                    )
                })
                .collect::<Vec<_>>(),
            &FACT_ORDER
                .iter()
                .map(|fact| (fact.clone(), proved.proves(fact) || facts.contains(fact)))
                .collect::<Vec<_>>(),
        )
    }

    // ---- around `linux` ----

    /// The one `linux` driver root this process's scope is; released when the adapter goes.
    struct LinuxHold;

    impl LinuxHold {
        fn take() -> Result<Self, ConfinementError> {
            LINUX_HELD
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map(|_| Self)
                .map_err(|_| {
                    unserved(
                        "this process's scope is already the delegated root of an open systemd \
                         adapter around linux",
                    )
                })
        }
    }

    impl Drop for LinuxHold {
        fn drop(&mut self) {
            LINUX_HELD.store(false, Ordering::Release);
        }
    }

    fn open_linux(
        manager: &Manager,
        request: super::LinuxRequest,
    ) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
        let hold = LinuxHold::take()?;
        let scope = process_scope(manager)?;
        let inner = super::super::linux::open(request.with_cgroup_root(Some(scope.clone())))
            .map_err(|error| match error {
                ConfinementError::AdapterUnserved { detail, .. } => {
                    unserved(format!("linux in {}: {detail}", scope.display()))
                }
                other => other,
            })?;
        let proved = inner.facts();
        if !proved.serves(&ContractFamily::Limits) || !proved.proves(&Fact::WholeTreeKill) {
            return Err(unserved(format!(
                "linux did not prove limits and whole_tree_kill with {} as its root",
                scope.display()
            )));
        }
        Ok(Arc::new(SystemdLinux {
            facts: proof(InnerAdapter::Linux, proved, &[], &[]),
            inner,
            _hold: hold,
        }))
    }

    struct SystemdLinux {
        facts: AdapterFacts,
        inner: Arc<dyn ConfinementAdapter>,
        _hold: LinuxHold,
    }

    impl SystemdLinux {
        /// The inner `linux` admission of a profile this adapter admitted: the same profile, with
        /// no `inner` and no audit sink, since this adapter writes the records.
        fn inner_admission(
            &self,
            admitted: &AdmittedProfile,
            running: bool,
        ) -> Result<AdmittedProfile, ConfinementError> {
            admitted.check_on(&self.facts, running)?;
            let mut profile = admitted.profile().clone();
            profile.inner = None;
            profile.audit = None;
            self.inner.admit(&profile)
        }

        fn relabel(mut observation: RunObservation) -> RunObservation {
            observation.adapter = AdapterName::Systemd;
            observation.inner = Some(InnerAdapter::Linux);
            observation
        }
    }

    #[async_trait]
    impl ConfinementAdapter for SystemdLinux {
        fn facts(&self) -> &AdapterFacts {
            &self.facts
        }

        fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
            self.inner.check(profile)
        }

        fn render(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RenderedCommand, ConfinementError> {
            let inner = self.inner_admission(admitted, false)?;
            self.inner.render(&inner, command)
        }

        async fn start(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RunId, ConfinementError> {
            let inner = self.inner_admission(admitted, true)?;
            let run = self.inner.start(&inner, command).await?;
            admitted.audit_start(&run, &command.argv[0]);
            Ok(run)
        }

        async fn observe(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
            self.inner.observe(run).await.map(Self::relabel)
        }

        async fn output(&self, run: &RunId) -> Result<RunOutput, ConfinementError> {
            self.inner.output(run).await
        }

        async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
            self.inner.wait(run).await.map(Self::relabel)
        }

        async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
            self.inner.signal(run, signal).await
        }
    }

    // ---- around `local` ----

    fn open_local(
        manager: &Manager,
        request: super::LocalRequest,
    ) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
        let local = super::super::local::open(request);
        let scope = process_scope(manager)?;
        Ok(Arc::new(SystemdLocal {
            facts: proof(
                InnerAdapter::Local,
                local.facts(),
                &[ContractFamily::Limits],
                &[Fact::WholeTreeKill],
            ),
            local,
            scope,
            runs: Mutex::new(Vec::new()),
        }))
    }

    #[derive(Debug, Clone, Copy)]
    enum Phase {
        Running,
        Ended(RunExit),
        /// The leader ended, and the tree did not leave its cgroup in time.
        Unknown,
    }

    #[derive(Default)]
    struct Captured {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_truncated: bool,
        stderr_truncated: bool,
    }

    struct LocalRun {
        id: RunId,
        pid: i32,
        reason: Option<String>,
        cgroup: PathBuf,
        phase: watch::Receiver<Phase>,
        captured: Arc<Mutex<Captured>>,
        /// Set, under this lock, after the leader exited and before it is reaped: while it is
        /// false the leader's pid still names this run's process group.
        leader_exited: Arc<Mutex<bool>>,
    }

    struct SystemdLocal {
        facts: AdapterFacts,
        local: Arc<dyn ConfinementAdapter>,
        scope: PathBuf,
        runs: Mutex<Vec<Arc<LocalRun>>>,
    }

    /// The profile's bounds, refused `exec.limit-unserved` outside what the scope enforces, the
    /// same ranges the `linux` driver admits.
    fn bounds(profile: &ConfinementProfile) -> Result<(ProcessBounds, usize), ConfinementError> {
        let bounds = profile.process_bounds.unwrap_or(DEFAULT_BOUNDS);
        let output = profile
            .io_bounds
            .map_or(Ok(SYSTEMD_OUTPUT_LIMIT_BYTES), |io| {
                usize::try_from(io.output_bytes)
            })
            .unwrap_or(usize::MAX);
        if !(1..=86_400_000).contains(&bounds.timeout_ms)
            || !(1..=4096).contains(&bounds.processes)
            || !(1_048_576..=1_099_511_627_776).contains(&bounds.memory_bytes)
            || !(1..=86_400_000).contains(&bounds.cpu_millis)
            || !(1..=SYSTEMD_OUTPUT_LIMIT_BYTES).contains(&output)
        {
            return Err(DriverError::exhausted(
                "exec.limit-unserved",
                "Exec bounds exceed what the systemd adapter enforces.",
                "limits",
            )
            .into());
        }
        Ok((bounds, output))
    }

    /// Kills every process in `cgroup`. A cgroup already gone has nothing left to kill.
    fn kill_tree(cgroup: &Path) -> std::io::Result<()> {
        match std::fs::write(cgroup.join("cgroup.kill"), "1") {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn cpu_usage_micros(cgroup: &Path) -> Option<u64> {
        std::fs::read_to_string(cgroup.join("cpu.stat"))
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("usage_usec "))?
            .trim()
            .parse()
            .ok()
    }

    /// Kills what is left in `cgroup` until it is empty, then removes it. False when the tree did
    /// not leave within [`TREE_DRAIN`].
    async fn drain_tree(cgroup: &Path) -> bool {
        let started = Instant::now();
        loop {
            let procs = match std::fs::read_to_string(cgroup.join("cgroup.procs")) {
                Ok(procs) => procs,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
                Err(_) => String::from("unreadable"),
            };
            if procs.trim().is_empty() && std::fs::remove_dir(cgroup).is_ok() {
                return true;
            }
            if started.elapsed() > TREE_DRAIN {
                return procs.trim().is_empty();
            }
            let _ = kill_tree(cgroup);
            tokio::time::sleep(POLL).await;
        }
    }

    /// Makes a run's cgroup beneath the scope and writes its bounds.
    fn create_run_cgroup(cgroup: &Path, bounds: &ProcessBounds) -> Result<(), ConfinementError> {
        std::fs::create_dir(cgroup).map_err(|error| {
            DriverError::failed(
                "exec.cgroup-create-failed",
                format!("systemd run cgroup {}: {error}", cgroup.display()),
            )
        })?;
        let written = [
            ("pids.max", bounds.processes.to_string()),
            ("memory.max", bounds.memory_bytes.to_string()),
            ("memory.swap.max", "0".to_owned()),
            // A memory bound ends the tree, not the one process the kernel picked.
            ("memory.oom.group", "1".to_owned()),
        ]
        .iter()
        .try_for_each(|(name, value)| std::fs::write(cgroup.join(name), value));
        if let Err(error) = written {
            let _ = std::fs::remove_dir(cgroup);
            return Err(DriverError::failed(
                "exec.cgroup-create-failed",
                format!("systemd run cgroup {}: {error}", cgroup.display()),
            )
            .into());
        }
        Ok(())
    }

    /// Spawns `command` in `directory` as its own process group, joined to `cgroup` before it
    /// executes.
    fn spawn_in(
        cgroup: &Path,
        command: &Command,
        directory: &Path,
    ) -> Result<tokio::process::Child, ConfinementError> {
        let procs = std::fs::OpenOptions::new()
            .write(true)
            .open(cgroup.join("cgroup.procs"))
            .map_err(|error| {
                DriverError::failed(
                    "exec.cgroup-create-failed",
                    format!("systemd run cgroup {}: {error}", cgroup.display()),
                )
            })?;
        let procs_fd = procs.as_raw_fd();
        let mut child = tokio::process::Command::new(&command.argv[0]);
        child
            .args(&command.argv[1..])
            .env_clear()
            .envs(&command.env)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: the hook only calls `write(2)`, which is async-signal-safe, on a descriptor the
        // parent keeps open until `spawn` returns. Writing `0` to `cgroup.procs` moves the writer:
        // the run is in its bounded cgroup before it executes, so nothing it forks starts outside
        // it, and a run that cannot join is never executed.
        unsafe {
            child.pre_exec(move || {
                let written = libc::write(procs_fd, b"0".as_ptr().cast(), 1);
                if written == 1 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let spawned = child.spawn();
        drop(procs);
        spawned.map_err(|error| {
            DriverError::failed("exec.spawn-failed", format!("systemd spawn: {error}")).into()
        })
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

    async fn drain(
        mut stream: impl AsyncRead + Unpin,
        captured: Arc<Mutex<Captured>>,
        stdout: bool,
        limit: usize,
    ) {
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
            let room = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..read.min(room)]);
            *truncated |= read > room;
        }
    }

    impl SystemdLocal {
        fn run(&self, id: &RunId) -> Result<Arc<LocalRun>, ConfinementError> {
            self.runs
                .lock()
                .iter()
                .find(|run| &run.id == id)
                .cloned()
                .ok_or_else(super::super::unknown_run)
        }

        fn observation(run: &LocalRun) -> RunObservation {
            let (state, exit) = match *run.phase.borrow() {
                Phase::Running => (RunState::Running, None),
                Phase::Ended(exit) => (RunState::Ended, Some(exit)),
                Phase::Unknown => (RunState::Unknown, None),
            };
            RunObservation {
                run: run.id.clone(),
                adapter: AdapterName::Systemd,
                inner: Some(InnerAdapter::Local),
                declared_insecure: run.reason.clone(),
                state,
                exit,
            }
        }

        /// Makes room for one more run, forgetting ended runs oldest first.
        fn reserve(&self) -> Result<(), ConfinementError> {
            let mut runs = self.runs.lock();
            while runs.len() >= SYSTEMD_MAX_TRACKED_RUNS {
                let Some(ended) = runs
                    .iter()
                    .position(|run| !matches!(*run.phase.borrow(), Phase::Running))
                else {
                    return Err(DriverError::exhausted(
                        "exec.tracking-capacity",
                        "Every tracked systemd run is still running.",
                        "run",
                    )
                    .into());
                };
                runs.remove(ended);
            }
            Ok(())
        }

        fn directory(admitted: &AdmittedProfile) -> Result<PathBuf, ConfinementError> {
            let workspace = super::super::workspace_of(admitted)?;
            if !workspace.is_dir() {
                return Err(DriverError::not_found().into());
            }
            let Some(workdir) = &admitted.profile().workdir else {
                return Ok(workspace.clone());
            };
            let directory = workspace.join(super::super::relative_workdir(workdir)?);
            if !directory.is_dir() {
                return Err(DriverError::refused(
                    substrate_wire::EXEC_WORKDIR_MISSING,
                    "The working directory is not a directory beneath the workspace.",
                    "workdir",
                )
                .into());
            }
            Ok(directory)
        }

        /// Ends the tree at the run's timeout or once its cumulative CPU passes its budget.
        fn watch(cgroup: PathBuf, bounds: ProcessBounds, mut phase: watch::Receiver<Phase>) {
            let deadline = Instant::now() + Duration::from_millis(bounds.timeout_ms);
            let budget = bounds.cpu_millis.saturating_mul(1_000);
            tokio::spawn(async move {
                loop {
                    let stop = tokio::time::timeout(
                        WATCH,
                        phase.wait_for(|phase| !matches!(phase, Phase::Running)),
                    )
                    .await;
                    if stop.is_ok() {
                        return;
                    }
                    if Instant::now() >= deadline
                        || cpu_usage_micros(&cgroup).is_some_and(|used| used >= budget)
                    {
                        let _ = kill_tree(&cgroup);
                        return;
                    }
                }
            });
        }
    }

    #[async_trait]
    impl ConfinementAdapter for SystemdLocal {
        fn facts(&self) -> &AdapterFacts {
            &self.facts
        }

        /// `local`'s own refusals, and bounds outside what the scope enforces.
        fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
            self.local.check(profile)?;
            bounds(profile).map(|_| ())
        }

        fn render(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RenderedCommand, ConfinementError> {
            admitted.check_on(&self.facts, false)?;
            super::super::check_command(command)?;
            Ok(RenderedCommand {
                program: PathBuf::from(&command.argv[0]),
                args: command.argv[1..].to_vec(),
            })
        }

        async fn start(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RunId, ConfinementError> {
            admitted.check_on(&self.facts, true)?;
            super::super::check_command(command)?;
            let directory = Self::directory(admitted)?;
            let (bounds, output_limit) = bounds(admitted.profile())?;
            self.reserve()?;
            let name = format!(
                "local-{}",
                ulid::Ulid::generate().to_string().to_ascii_lowercase()
            );
            let cgroup = self.scope.join(&name);
            create_run_cgroup(&cgroup, &bounds)?;
            let mut child = spawn_in(&cgroup, command, &directory).inspect_err(|_| {
                let _ = std::fs::remove_dir(&cgroup);
            })?;
            let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
                let _ = kill_tree(&cgroup);
                return Err(
                    DriverError::failed("exec.spawn-failed", "systemd spawn: no pid").into(),
                );
            };
            let id = RunId::new(format!("systemd-{name}"));
            let captured = Arc::new(Mutex::new(Captured::default()));
            let (phase, observed) = watch::channel(Phase::Running);
            let stdout = child.stdout.take().map(|stream| {
                tokio::spawn(drain(stream, Arc::clone(&captured), true, output_limit))
            });
            let stderr = child.stderr.take().map(|stream| {
                tokio::spawn(drain(stream, Arc::clone(&captured), false, output_limit))
            });
            let leader_exited = Arc::new(Mutex::new(false));
            let exited = Arc::clone(&leader_exited);
            let tree = cgroup.clone();
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || await_exit_unreaped(pid)).await;
                *exited.lock() = true;
                // Nothing of the tree outlives its leader.
                let _ = kill_tree(&tree);
                let ended = child.wait().await;
                for reader in [stdout, stderr].into_iter().flatten() {
                    let _ = reader.await;
                }
                let drained = drain_tree(&tree).await;
                let _ = phase.send(match ended {
                    Ok(status) if drained => Phase::Ended(RunExit {
                        code: status.code(),
                        signal: status.signal(),
                    }),
                    _ => Phase::Unknown,
                });
            });
            Self::watch(cgroup.clone(), bounds, observed.clone());
            self.runs.lock().push(Arc::new(LocalRun {
                id: id.clone(),
                pid,
                reason: admitted.insecure_reason().map(str::to_owned),
                cgroup,
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
            // The sender lives until the tree has left its cgroup, and it always sends first.
            let _ = phase
                .wait_for(|phase| !matches!(phase, Phase::Running))
                .await;
            Ok(Self::observation(&run))
        }

        /// `KILL` ends the whole tree through `cgroup.kill`; another signal reaches the run's
        /// process group, as under `local`.
        async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
            let run = self.run(run)?;
            if signal == Signal::Kill {
                return kill_tree(&run.cgroup).map_err(|error| {
                    DriverError::failed("exec.signal-failed", format!("systemd kill: {error}"))
                        .into()
                });
            }
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
                        format!("systemd signal: {error}"),
                    )
                    .into());
                }
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::super::{IoBounds, local};
        use super::*;

        #[test]
        fn around_local_the_scope_adds_limits_and_whole_tree_kill_only() {
            let facts = proof(
                InnerAdapter::Local,
                &local::proof(true),
                &[ContractFamily::Limits],
                &[Fact::WholeTreeKill],
            );
            assert_eq!(facts.adapter, AdapterName::Systemd);
            assert_eq!(facts.inner, Some(InnerAdapter::Local));
            assert_eq!(
                facts.served_families,
                [
                    ContractFamily::ProcessRun,
                    ContractFamily::ProcessWatch,
                    ContractFamily::OutputRead,
                    ContractFamily::Wait,
                    ContractFamily::Workspace,
                    ContractFamily::Limits,
                    ContractFamily::Audit,
                ]
            );
            assert_eq!(
                facts.facts,
                [
                    Fact::DeclaredInsecure,
                    Fact::PathsAreHostPaths,
                    Fact::WholeTreeKill,
                    Fact::LocalPermitted,
                    Fact::HostNetwork,
                ]
            );
            assert!(super::super::super::check_open(&facts).is_ok());
        }

        #[test]
        fn bounds_outside_what_the_scope_enforces_are_refused() {
            let profile = |process: ProcessBounds, output_bytes: u64| ConfinementProfile {
                process_bounds: Some(process),
                io_bounds: Some(IoBounds { output_bytes }),
                ..ConfinementProfile::default()
            };
            assert!(bounds(&profile(DEFAULT_BOUNDS, 1024)).is_ok());
            assert!(bounds(&ConfinementProfile::default()).is_ok());
            for (process, output) in [
                (
                    ProcessBounds {
                        processes: 0,
                        ..DEFAULT_BOUNDS
                    },
                    1024,
                ),
                (
                    ProcessBounds {
                        memory_bytes: 1024,
                        ..DEFAULT_BOUNDS
                    },
                    1024,
                ),
                (
                    ProcessBounds {
                        timeout_ms: 0,
                        ..DEFAULT_BOUNDS
                    },
                    1024,
                ),
                (
                    ProcessBounds {
                        cpu_millis: 0,
                        ..DEFAULT_BOUNDS
                    },
                    1024,
                ),
                (DEFAULT_BOUNDS, 0),
                (DEFAULT_BOUNDS, SYSTEMD_OUTPUT_LIMIT_BYTES as u64 + 1),
            ] {
                let error = bounds(&profile(process, output)).expect_err("refused");
                assert_eq!(error.code(), "exec.limit-unserved", "{process:?} {output}");
            }
        }

        #[test]
        fn no_session_bus_is_unserved_before_anything_runs() {
            for bus in [None, Some(OsString::new())] {
                let error = Manager::reach(bus).err().expect("unserved");
                assert_eq!(error.code(), "confinement.adapter-unserved", "{error}");
            }
        }
    }
}
