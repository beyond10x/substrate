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
//! The same holds for a run that reaches the user manager. A start around `local` whose
//! environment names `DBUS_SESSION_BUS_ADDRESS`, `DBUS_STARTER_ADDRESS` or `XDG_RUNTIME_DIR`, the
//! variables a client finds the user manager through, is refused `exec.environment-refused`. A run
//! can still reach the manager another way (the socket's path is no secret) and have it move a
//! descendant out of the run's cgroup, where `cgroup.kill` does not reach it. The timeout, the CPU
//! budget, `KILL` and the leader's own exit therefore also kill the leader's process group, which
//! ends a descendant that stayed in it; one that left both the cgroup and the group outlives the
//! run, and `wait` reports the run `Unknown` once ten seconds past the leader's exit passed with
//! that descendant still holding the run's output or the cgroup still populated. `whole_tree_kill` around `local` is a claim about a
//! tree that does not ask the user manager to move it.
//!
//! The leader dies with the thread that started it (`PR_SET_PDEATHSIG`), as `linux`'s
//! `--die-with-parent` does; the timeout and the CPU budget are enforced by this process, so a
//! descendant that outlives a dead embedder is unbounded in time, though still in its bounded
//! cgroup, until the next `open` of `systemd` by any process of the same user. Every `open` kills
//! (`cgroup.kill`) and stops each `substrate-confinement-*` scope that is active, was not made by
//! this process and holds no embedder (its root and its `embedder` leaf hold no process), and adds
//! the count to [`reclaimed_scopes`].
//!
//! Opening `systemd` moves this process out of the cgroup it was started in, which includes an
//! operator's delegated cgroup root: a `linux` adapter opened afterwards with that root does not
//! prove `limits` and refuses a profile that needs them `confinement.family-unserved`. An embedder
//! that opened `systemd` and needs a bounded confined run uses `systemd` with `inner: linux`. The
//! scope carries the nearest `pids.max` and `memory.max` bound of the cgroup the process left, as
//! its `TasksMax` and `MemoryMax`, so the move takes the process out of no bound it was under.
//!
//! The open is refused `confinement.adapter-unserved`, before anything moves, when no user manager
//! is reachable: `DBUS_SESSION_BUS_ADDRESS` unset, empty or not one `unix:path=` address (an
//! address of another form can name a program to run), no `systemctl` or `busctl`, or
//! `systemctl --user` not answering; and when the enclosing bounds cannot be read. It is refused
//! the same when the scope the manager made does not carry `cpu`, `memory` and `pids`,
//! `cgroup.kill` or the enclosing bounds, or when `linux` did not prove `limits` and
//! `whole_tree_kill` in it: a `systemd` adapter that cannot bound a run is not opened (invariant 3).

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

/// The variables through which a client finds the user manager; a start around `local` that names
/// one is refused `exec.environment-refused`.
pub const USER_MANAGER_ENV: [&str; 3] =
    [SESSION_BUS_ENV, "DBUS_STARTER_ADDRESS", "XDG_RUNTIME_DIR"];

/// How many leftover `substrate-confinement-*` scopes the opens of this process have killed and
/// stopped, summed.
pub fn reclaimed_scopes() -> usize {
    #[cfg(target_os = "linux")]
    {
        imp::reclaimed()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use parking_lot::Mutex;
    use substrate_wire::Signal;
    use tokio::sync::watch;

    use super::super::live::{self, LiveOutput};
    use super::super::stdin::{self, StdinWriter};
    use super::super::{
        AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
        ConfinementProfile, ContractFamily, FACT_ORDER, FAMILY_ORDER, Fact, InnerAdapter,
        OutputEvent, ProcessBounds, RenderedCommand, RunExit, RunId, RunLimit, RunObservation,
        RunOutput, RunState, named_limit,
    };
    use super::{
        InnerRequest, SESSION_BUS_ENV, SYSTEMD_MAX_TRACKED_RUNS, SYSTEMD_OUTPUT_LIMIT_BYTES,
        SystemdRequest, USER_MANAGER_ENV,
    };
    use crate::DriverError;
    use crate::output_retention::{Captured, drain};

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
    /// Every scope unit this process made: never another embedder's leftover, even once this
    /// process has left it.
    static OWN_UNITS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Leftover scopes this process's opens killed and stopped.
    static RECLAIMED: AtomicUsize = AtomicUsize::new(0);
    const UNIT_PREFIX: &str = "substrate-confinement-";

    pub(super) fn reclaimed() -> usize {
        RECLAIMED.load(Ordering::Relaxed)
    }

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

    /// Whether `bus` is exactly one `unix:path=` address: the only form that names a socket and
    /// never a program to run (`unixexec:`) or a remote peer.
    fn is_unix_path_address(bus: &std::ffi::OsStr) -> bool {
        bus.to_str()
            .and_then(|bus| bus.strip_prefix("unix:path="))
            .is_some_and(|rest| !rest.is_empty() && !rest.contains(';'))
    }

    /// The nearest bound in `file` (`pids.max`, `memory.max`) on `cgroup` or an ancestor of it
    /// beneath `root`: `None` when every level that carries the file says `max`. An error when no
    /// level carries it, or one cannot be read.
    fn enclosing_bound(root: &Path, cgroup: &Path, file: &str) -> Result<Option<u64>, String> {
        let mut carried = false;
        for level in cgroup
            .ancestors()
            .take_while(|level| level.starts_with(root) && *level != root)
        {
            let value = match std::fs::read_to_string(level.join(file)) {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("{}: {error}", level.join(file).display())),
            };
            carried = true;
            let value = value.trim();
            if value != "max" {
                return value
                    .parse()
                    .map(Some)
                    .map_err(|_| format!("{}: {value:?}", level.join(file).display()));
            }
        }
        if carried {
            Ok(None)
        } else {
            Err(format!(
                "no cgroup enclosing {} carries {file}",
                cgroup.display()
            ))
        }
    }

    /// The bounds the scope must carry: those of the cgroup this process is in now.
    struct Enclosing {
        tasks: Option<u64>,
        memory: Option<u64>,
    }

    impl Enclosing {
        fn read() -> Result<Self, ConfinementError> {
            let current = own_cgroup().ok_or_else(|| {
                unserved("this process's cgroup cannot be read, nor the bounds it is under")
            })?;
            let bound = |file: &str| {
                enclosing_bound(Path::new(CGROUP_FS), &current, file).map_err(|detail| {
                    unserved(format!(
                        "the bounds this process is under cannot be read: {detail}"
                    ))
                })
            };
            Ok(Self {
                tasks: bound("pids.max")?,
                memory: bound("memory.max")?,
            })
        }

        /// The value systemd takes for the property: `u64::MAX` is no bound.
        fn property(bound: Option<u64>) -> String {
            bound.unwrap_or(u64::MAX).to_string()
        }

        /// Refuses a scope whose cgroup does not carry these bounds.
        fn check(&self, scope: &Path, unit: &str) -> Result<(), ConfinementError> {
            for (file, bound) in [("pids.max", self.tasks), ("memory.max", self.memory)] {
                let carried = std::fs::read_to_string(scope.join(file)).unwrap_or_default();
                let wanted = bound.map_or_else(|| "max".to_owned(), |bound| bound.to_string());
                if carried.trim() != wanted {
                    return Err(unserved(format!(
                        "{unit} carries {file} {:?}, not the enclosing {wanted}",
                        carried.trim()
                    )));
                }
            }
            Ok(())
        }
    }

    /// Whether `cgroup` holds no process of its own; a cgroup that is not there holds none, and
    /// one that cannot be read is taken to hold one.
    fn holds_no_process(cgroup: &Path) -> bool {
        match std::fs::read_to_string(cgroup.join("cgroup.procs")) {
            Ok(procs) => procs.trim().is_empty(),
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    }

    /// The user manager, reached.
    struct Manager {
        bus: OsString,
        busctl: PathBuf,
        systemctl: PathBuf,
    }

    impl Manager {
        fn reach(bus: Option<OsString>) -> Result<Self, ConfinementError> {
            let bus = bus.filter(|bus| !bus.is_empty()).ok_or_else(|| {
                unserved(format!(
                    "{SESSION_BUS_ENV} is unset: no user systemd manager is reachable"
                ))
            })?;
            if !is_unix_path_address(&bus) {
                return Err(unserved(format!(
                    "{SESSION_BUS_ENV} is not one unix:path= address"
                )));
            }
            let systemctl = tool("systemctl").ok_or_else(|| unserved("no systemctl"))?;
            let busctl = tool("busctl").ok_or_else(|| unserved("no busctl"))?;
            let manager = Self {
                bus,
                busctl,
                systemctl,
            };
            manager.call(
                &manager.systemctl,
                &["--user", "show", "--property=Version", "--value"],
            )?;
            Ok(manager)
        }

        /// Kills and stops every active `substrate-confinement-*` scope this process did not make
        /// whose root and `embedder` leaf hold no process: what an embedder that died left. A
        /// scope still being entered holds its embedder in its root and is left alone.
        fn reclaim(&self) -> Result<usize, ConfinementError> {
            let listed = self.call(
                &self.systemctl,
                &[
                    "--user",
                    "list-units",
                    "--all",
                    "--plain",
                    "--no-legend",
                    "--type=scope",
                    "substrate-confinement-*",
                ],
            )?;
            let own = OWN_UNITS.lock().clone();
            let mut reclaimed = 0;
            for unit in listed
                .lines()
                .filter_map(|line| line.split_whitespace().next())
            {
                let named = unit
                    .strip_prefix(UNIT_PREFIX)
                    .and_then(|rest| rest.strip_suffix(".scope"))
                    .is_some_and(|id| {
                        !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())
                    });
                if !named || own.iter().any(|mine| mine == unit) {
                    continue;
                }
                let Ok(shown) = self.call(
                    &self.systemctl,
                    &[
                        "--user",
                        "show",
                        "--property=ActiveState",
                        "--property=ControlGroup",
                        unit,
                    ],
                ) else {
                    continue;
                };
                let property = |name: &str| {
                    shown
                        .lines()
                        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
                        .unwrap_or_default()
                };
                let control_group = property("ControlGroup");
                if property("ActiveState") != "active" || !control_group.starts_with('/') {
                    continue;
                }
                let scope = Path::new(CGROUP_FS).join(control_group.trim_start_matches('/'));
                if !holds_no_process(&scope) || !holds_no_process(&scope.join(EMBEDDER)) {
                    continue;
                }
                if kill_tree(&scope).is_err() {
                    continue;
                }
                // Emptied, the scope is usually stopped and collected by the manager before this
                // call, which then has no unit to stop.
                let _ = self.call(&self.systemctl, &["--user", "stop", unit]);
                reclaimed += 1;
            }
            if reclaimed > 0 {
                RECLAIMED.fetch_add(reclaimed, Ordering::Relaxed);
                tracing::warn!(
                    reclaimed,
                    "systemd confinement: killed and stopped leftover scopes of embedders that died"
                );
            }
            Ok(reclaimed)
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
            let enclosing = Enclosing::read()?;
            let tasks = Enclosing::property(enclosing.tasks);
            let memory = Enclosing::property(enclosing.memory);
            OWN_UNITS.lock().push(unit.clone());
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
                    "5",
                    "PIDs",
                    "au",
                    "1",
                    &pid,
                    "Delegate",
                    "b",
                    "true",
                    // The bounds this process was under when it moved; each run's own cgroup
                    // carries the run's bounds beneath them.
                    "TasksMax",
                    "t",
                    &tasks,
                    "MemoryMax",
                    "t",
                    &memory,
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
            enclosing.check(&scope, &unit)?;
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
        manager.reclaim()?;
        match request.inner {
            InnerRequest::Local(local) => open_local(&manager, local),
            InnerRequest::Linux(linux) => open_linux(&manager, *linux),
        }
    }

    /// The facts `systemd` around `inner` proves: `inner`'s, plus what the scope adds. What the
    /// inner adapter withheld at its open, the scope withholds for the same cause (design 35
    /// D27).
    fn proof(
        inner: InnerAdapter,
        proved: &AdapterFacts,
        families: &[ContractFamily],
        facts: &[Fact],
    ) -> AdapterFacts {
        let mut around = AdapterFacts::new(
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
        );
        around.withheld.clone_from(&proved.withheld);
        around
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
        let inner =
            super::super::linux::open_adapter(request.with_cgroup_root(Some(scope.clone())))
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
        inner: Arc<super::super::linux::LinuxAdapter>,
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
            // Every refusal, from the contract's recheck on, writes its audit record.
            let started: Result<RunId, ConfinementError> = async {
                let inner = self.inner_admission(admitted, true)?;
                let run = self.inner.start(&inner, command).await?;
                let end = admitted.audit_start(&run, &command.argv[0]);
                // The inner admission has no sink, so the inner `linux` writes nothing: this
                // adapter's own task awaits the inner run's end and writes its `run_ended` with
                // this profile and the relabelled observation (design 36 D20).
                if !end.is_inert() {
                    let ended = self.inner.ended_observation(&run);
                    tokio::spawn(async move {
                        if let Some(observation) = ended.await {
                            end.write(Self::relabel(observation));
                        }
                    });
                }
                Ok(run)
            }
            .await;
            admitted.audit_refusal(&self.facts, started)
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

        async fn write_stdin(&self, run: &RunId, bytes: &[u8]) -> Result<(), ConfinementError> {
            self.inner.write_stdin(run, bytes).await
        }

        async fn close_stdin(&self, run: &RunId) -> Result<(), ConfinementError> {
            self.inner.close_stdin(run).await
        }

        async fn read_output(
            &self,
            run: &RunId,
            timeout: Duration,
        ) -> Result<Option<OutputEvent>, ConfinementError> {
            self.inner.read_output(run, timeout).await
        }

        fn output_stalled(&self, run: &RunId) -> bool {
            self.inner.output_stalled(run)
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
        Ended(RunExit, EndRead),
        /// The leader ended, and the tree did not leave its cgroup in time.
        Unknown(EndRead),
    }

    impl Phase {
        /// The phase of an ended leader: `Ended` with its status, or `Unknown` where its wait
        /// failed or its tree did not drain (`None`).
        fn at_end(status: Option<std::process::ExitStatus>, end: EndRead) -> Self {
            match status {
                Some(status) => Self::Ended(
                    RunExit {
                        code: status.code(),
                        signal: status.signal(),
                    },
                    end,
                ),
                None => Self::Unknown(end),
            }
        }
    }

    /// What the run's end task read of the run's end beside its exit, before it removed the
    /// run's cgroup (design 36 D7, D8).
    #[derive(Debug, Clone, Copy, Default)]
    struct EndRead {
        limit: Option<RunLimit>,
        memory_oom_kills: Option<u64>,
    }

    impl EndRead {
        /// Design 36 D7, in D4's order: the stall, an OOM kill in the run's cgroup, and the bound
        /// the watch ended the run for.
        fn of(
            stall: Option<&Arc<LiveOutput>>,
            memory_oom_kills: Option<u64>,
            imposed: Option<RunLimit>,
        ) -> Self {
            let reached: Vec<RunLimit> = [
                live::stall_limit(stall),
                memory_oom_kills
                    .is_some_and(|kills| kills > 0)
                    .then_some(RunLimit::Memory),
                imposed,
            ]
            .into_iter()
            .flatten()
            .collect();
            Self {
                limit: named_limit(&reached),
                memory_oom_kills,
            }
        }
    }

    /// `memory.events` `oom_kill` of a run's cgroup; `None` where it cannot be read (design 36
    /// D8: absent, never zero).
    fn oom_kills(cgroup: &Path) -> Option<u64> {
        std::fs::read_to_string(cgroup.join("memory.events"))
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("oom_kill "))?
            .trim()
            .parse()
            .ok()
    }

    struct LocalRun {
        id: RunId,
        leader: Leader,
        reason: Option<String>,
        cgroup: PathBuf,
        phase: watch::Receiver<Phase>,
        captured: Arc<Mutex<Captured>>,
        /// The run's standard input, when its profile piped it (design 32).
        stdin: Option<Arc<StdinWriter>>,
        /// The run's live output, when its profile asked for it (design 32).
        live: Option<Arc<LiveOutput>>,
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

    /// Whether a process is left anywhere beneath `cgroup`, its own cgroups included, from
    /// `cgroup.events`: `None` when the cgroup is gone; one that cannot be read is populated.
    fn populated(cgroup: &Path) -> Option<bool> {
        match std::fs::read_to_string(cgroup.join("cgroup.events")) {
            Ok(events) => Some(!events.lines().any(|line| line.trim() == "populated 0")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(true),
        }
    }

    /// Removes `cgroup` and every cgroup a run made beneath it, deepest first.
    fn remove_subtree(cgroup: &Path) -> std::io::Result<()> {
        let mut pending = vec![(cgroup.to_path_buf(), false)];
        while let Some((directory, visited)) = pending.pop() {
            if visited {
                match std::fs::remove_dir(&directory) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error);
                    }
                    _ => continue,
                }
            }
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            pending.push((directory, true));
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    pending.push((entry.path(), false));
                }
            }
        }
        Ok(())
    }

    /// Kills what is left beneath `cgroup` until `cgroup.events` says `populated 0`, then removes
    /// it and every cgroup beneath it. False when a process was still left at `deadline`.
    async fn drain_tree(cgroup: &Path, deadline: tokio::time::Instant) -> bool {
        loop {
            match populated(cgroup) {
                None => return true,
                Some(false) => {
                    if remove_subtree(cgroup).is_ok() || tokio::time::Instant::now() >= deadline {
                        return true;
                    }
                }
                Some(true) => {
                    if tokio::time::Instant::now() >= deadline {
                        return false;
                    }
                    let _ = kill_tree(cgroup);
                }
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// A run's leader, which is also its process group: what a descendant the user manager moved
    /// out of the run's cgroup may still be in.
    #[derive(Clone)]
    struct Leader {
        pid: i32,
        /// Set, under this lock, after the leader exited and before it is reaped: while it is
        /// false the leader's pid still names this run's process group.
        exited: Arc<Mutex<bool>>,
        /// Set under `exited`'s lock when the embedder's `KILL` ends the run, before the signal:
        /// the watch, which decides under the same lock, then imposes nothing.
        stopped: Arc<AtomicBool>,
    }

    impl Leader {
        fn new(pid: i32) -> Self {
            Self {
                pid,
                exited: Arc::new(Mutex::new(false)),
                stopped: Arc::new(AtomicBool::new(false)),
            }
        }

        /// The embedder's `KILL`: the run's tree through `cgroup.kill` and the leader's group,
        /// recorded first under the lock the watch decides under, so the run names no limit. The
        /// lock is held across `kill`, as in every end below, so the run's task cannot reap the
        /// leader meanwhile and the pid cannot name another process.
        fn stop(&self, cgroup: &Path) -> std::io::Result<()> {
            let exited = self.exited.lock();
            self.stopped.store(true, Ordering::Release);
            let killed = kill_tree(cgroup);
            if !*exited {
                kill_group(self.pid);
            }
            killed
        }

        /// The stall's end ([`live::Kill`]): the tree and the leader's group, as `stop` ends
        /// them. Answers whether the leader had neither exited nor been stopped by the embedder,
        /// decided under the lock `stop` records under, so the stall ended the run (D1, D5).
        fn end_stalled(&self, cgroup: &Path) -> bool {
            let exited = self.exited.lock();
            let running = !*exited
                && !self.stopped.load(Ordering::Acquire)
                && !live::exited_unreaped(self.pid);
            let _ = kill_tree(cgroup);
            if !*exited {
                kill_group(self.pid);
            }
            running
        }

        /// The watch's end for `limit` (design 36 D7): decided under the lock, and only while the
        /// leader has neither exited, by itself or its unreaped exit, nor been stopped by the
        /// embedder. Then `limit` is recorded in `imposed` before live delivery stops and the
        /// tree and the leader's group are killed.
        fn impose(
            &self,
            limit: RunLimit,
            imposed: &Mutex<Option<RunLimit>>,
            live: Option<&Arc<LiveOutput>>,
            cgroup: &Path,
        ) {
            let exited = self.exited.lock();
            if *exited || self.stopped.load(Ordering::Acquire) || live::exited_unreaped(self.pid) {
                return;
            }
            *imposed.lock() = Some(limit);
            if let Some(live) = live {
                live.close();
            }
            let _ = kill_tree(cgroup);
            kill_group(self.pid);
        }
    }

    /// `KILL` to `pid` and its process group. Only for a leader that is not reaped yet.
    fn kill_group(pid: i32) {
        // SAFETY: `kill` signals processes; no memory is involved.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
            libc::kill(pid, libc::SIGKILL);
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
    /// executes, with the standard input `profile` asks for.
    fn spawn_in(
        cgroup: &Path,
        command: &Command,
        directory: &Path,
        profile: &ConfinementProfile,
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
        let parent = libc::pid_t::try_from(std::process::id()).map_err(|_| {
            DriverError::failed("exec.spawn-failed", "systemd spawn: this process's pid")
        })?;
        let death_signal = libc::c_ulong::try_from(libc::SIGKILL).unwrap_or(9);
        let mut child = tokio::process::Command::new(&command.argv[0]);
        child
            .args(&command.argv[1..])
            .env_clear()
            .envs(&command.env)
            .current_dir(directory)
            .stdin(stdin::stdio(profile.stdin))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: the hook only calls `write(2)`, `prctl(2)` and `getppid(2)`, which are
        // async-signal-safe, the first on a descriptor the parent keeps open until `spawn`
        // returns. Writing `0` to `cgroup.procs` moves the writer: the run is in its bounded
        // cgroup before it executes, so nothing it forks starts outside it, and a run that cannot
        // join is never executed. `PR_SET_PDEATHSIG` kills the leader when the thread that started
        // it ends, so a run does not outlive its embedder's timeout and budget; it survives
        // `execve`, and a parent already gone by then is refused rather than missed.
        unsafe {
            child.pre_exec(move || {
                if libc::write(procs_fd, b"0".as_ptr().cast(), 1) != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, death_signal, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
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
            Self::observation_of(&run.id, run.reason.as_deref(), *run.phase.borrow())
        }

        /// The observation of a run in `phase`: what `observe` and `wait` return, and what the
        /// run's end task writes as `run_ended` (design 36 D15).
        fn observation_of(id: &RunId, reason: Option<&str>, phase: Phase) -> RunObservation {
            let (state, exit, end) = match phase {
                Phase::Running => (RunState::Running, None, EndRead::default()),
                Phase::Ended(exit, end) => (RunState::Ended, Some(exit), end),
                Phase::Unknown(end) => (RunState::Unknown, None, end),
            };
            RunObservation {
                run: id.clone(),
                adapter: AdapterName::Systemd,
                inner: Some(InnerAdapter::Local),
                declared_insecure: reason.map(str::to_owned),
                state,
                exit,
                limit: end.limit,
                memory_oom_kills: end.memory_oom_kills,
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
            let relative = super::super::relative_workdir(workdir)?;
            let missing = || {
                ConfinementError::from(DriverError::refused(
                    substrate_wire::EXEC_WORKDIR_MISSING,
                    "The working directory is not a directory beneath the workspace.",
                    "workdir",
                ))
            };
            // Every component a directory itself, never a symbolic link, as `linux` and `local`
            // require (design 26).
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

        /// The environment's shape, and no variable through which the run finds the user
        /// manager: a run that asks it for a scope of its own leaves its bounds.
        fn check_environment(command: &Command) -> Result<(), ConfinementError> {
            crate::environment::check(&command.env)?;
            if let Some(name) = USER_MANAGER_ENV
                .iter()
                .find(|name| command.env.contains_key(**name))
            {
                return Err(DriverError::refused(
                    "exec.environment-refused",
                    format!(
                        "{name} reaches the user manager, which can move a process out of the \
                         run's bounds."
                    ),
                    "env",
                )
                .into());
            }
            Ok(())
        }

        /// Ends the tree, its cgroup and its leader's process group, once its cumulative CPU
        /// passes its budget or at the run's timeout: an imposed end, which stops live delivery
        /// first. The bound it ends the run for is written to `imposed` before the kill, and only
        /// while the leader has not exited and the embedder has not stopped it
        /// ([`Leader::impose`]); the CPU budget is asked first, so a tick in which both are true
        /// names `CpuTime` (design 36 D4).
        fn watch(
            cgroup: PathBuf,
            leader: Leader,
            bounds: ProcessBounds,
            mut phase: watch::Receiver<Phase>,
            live: Option<Arc<LiveOutput>>,
            imposed: Arc<Mutex<Option<RunLimit>>>,
        ) {
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
                    let reached = if cpu_usage_micros(&cgroup).is_some_and(|used| used >= budget) {
                        Some(RunLimit::CpuTime)
                    } else if Instant::now() >= deadline {
                        Some(RunLimit::WallTime)
                    } else {
                        None
                    };
                    // A leader that exited, or that the embedder stopped, is not ended by a
                    // bound: the watch then records nothing and leaves the end to the end task.
                    if let Some(limit) = reached {
                        leader.impose(limit, &imposed, live.as_ref(), &cgroup);
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
            Self::check_environment(command)?;
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
            // Every refusal, from the contract's recheck on, writes its audit record.
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
            // The sender lives until the tree has left its cgroup or the drain's deadline passed,
            // and it always sends first.
            let _ = phase
                .wait_for(|phase| !matches!(phase, Phase::Running))
                .await;
            Ok(Self::observation(&run))
        }

        /// `KILL` stops live delivery, then ends the whole tree through `cgroup.kill`, and the
        /// leader's process group; another signal reaches the run's process group, as under
        /// `local`, and closes nothing.
        async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
            let run = self.run(run)?;
            if signal == Signal::Kill {
                if let Some(live) = &run.live {
                    live.close();
                }
                return run.leader.stop(&run.cgroup).map_err(|error| {
                    DriverError::failed("exec.signal-failed", format!("systemd kill: {error}"))
                        .into()
                });
            }
            // Held across `kill`: the run's task cannot reap the leader meanwhile, so the group id
            // cannot have been reused by another process.
            let exited = run.leader.exited.lock();
            if *exited {
                return Ok(());
            }
            // SAFETY: `kill` with a negative pid signals one process group; no memory is involved.
            if unsafe { libc::kill(-run.leader.pid, signal.number()) } != 0 {
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

    impl SystemdLocal {
        /// Starts a run; called from `start`, inside the runtime its tasks are spawned on.
        fn start_run(
            &self,
            admitted: &AdmittedProfile,
            command: &Command,
        ) -> Result<RunId, ConfinementError> {
            admitted.check_on(&self.facts, true)?;
            super::super::check_command(command)?;
            Self::check_environment(command)?;
            let directory = Self::directory(admitted)?;
            let (bounds, output_limit) = bounds(admitted.profile())?;
            self.reserve()?;
            let name = format!(
                "local-{}",
                ulid::Ulid::generate().to_string().to_ascii_lowercase()
            );
            let cgroup = self.scope.join(&name);
            create_run_cgroup(&cgroup, &bounds)?;
            let mut child = spawn_in(&cgroup, command, &directory, admitted.profile())
                .inspect_err(|_| {
                    let _ = std::fs::remove_dir(&cgroup);
                })?;
            let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
                let _ = kill_tree(&cgroup);
                return Err(
                    DriverError::failed("exec.spawn-failed", "systemd spawn: no pid").into(),
                );
            };
            let id = RunId::new(format!("systemd-{name}"));
            let writer = stdin::child_writer(admitted.profile().stdin, child.stdin.take());
            // The run record owns the writer; the reaper only reaches it, so dropping the adapter
            // drops the pipe and the child reads end of file.
            let ending = writer.as_ref().map(Arc::downgrade);
            let captured = Arc::new(Mutex::new(Captured::new(
                admitted.profile().output_retention,
                output_limit,
            )));
            let (phase, observed) = watch::channel(Phase::Running);
            let leader = Leader::new(pid);
            // A stall ends the run as `signal(Kill)` does: the whole tree and the leader's group.
            let (live, [stdout_feed, stderr_feed]) =
                live::open(admitted.profile().output_delivery, || {
                    let cgroup = cgroup.clone();
                    let leader = leader.clone();
                    Box::new(move || leader.end_stalled(&cgroup))
                });
            let stdout = child.stdout.take().map(|stream| {
                tokio::spawn(drain(stream, Arc::clone(&captured), true, stdout_feed))
            });
            let stderr = child.stderr.take().map(|stream| {
                tokio::spawn(drain(stream, Arc::clone(&captured), false, stderr_feed))
            });
            let exited = Arc::clone(&leader.exited);
            let tree = cgroup.clone();
            let imposed = Arc::new(Mutex::new(None));
            let watched = Arc::clone(&imposed);
            let stall = live.clone();
            let reason = admitted.insecure_reason().map(str::to_owned);
            // The run's `run_ended` reaches its end task once `audit_start` made it (design 36
            // D17).
            let (hand_end, end_record) = tokio::sync::oneshot::channel();
            let (end_id, end_reason) = (id.clone(), reason.clone());
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || await_exit_unreaped(pid)).await;
                *exited.lock() = true;
                let deadline = tokio::time::Instant::now() + TREE_DRAIN;
                // Nothing of the tree outlives its leader: neither its cgroup nor, while the
                // unreaped leader still holds the group's id, its process group.
                let _ = kill_tree(&tree);
                kill_group(pid);
                let ended = child.wait().await;
                // A descendant outside both still holding the output is not waited past the
                // deadline: the run is then unknown.
                let mut closed = true;
                for mut reader in [stdout, stderr].into_iter().flatten() {
                    if tokio::time::timeout_at(deadline, &mut reader)
                        .await
                        .is_err()
                    {
                        reader.abort();
                        closed = false;
                    }
                }
                // Read before `drain_tree` removes the cgroup it lives in (design 36 D8).
                let memory_oom_kills = oom_kills(&tree);
                let drained = drain_tree(&tree, deadline).await && closed;
                // The run's end, for its standard input too (design 32 § 2).
                if let Some(writer) = ending.as_ref().and_then(std::sync::Weak::upgrade) {
                    writer.end().await;
                }
                let end = EndRead::of(stall.as_ref(), memory_oom_kills, *imposed.lock());
                let ended = Phase::at_end(ended.ok().filter(|_| drained), end);
                let _ = phase.send(ended);
                // After the end is published, so neither `wait` nor `observe` waits for it (D20).
                let observation = Self::observation_of(&end_id, end_reason.as_deref(), ended);
                super::super::write_run_end(end_record, observation).await;
            });
            Self::watch(
                cgroup.clone(),
                leader.clone(),
                bounds,
                observed.clone(),
                live.clone(),
                watched,
            );
            self.runs.lock().push(Arc::new(LocalRun {
                id: id.clone(),
                leader,
                reason,
                cgroup,
                phase: observed,
                captured,
                stdin: writer,
                live,
            }));
            let _ = hand_end.send(admitted.audit_start(&id, &command.argv[0]));
            Ok(id)
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
                    Fact::StdinPipe,
                    Fact::OutputLive,
                ]
            );
            assert!(super::super::super::check_open(&facts).is_ok());
        }

        /// Design 35 D27: `systemd` around a `linux` whose open withheld a fact carries the same
        /// entries and proves no more than the inner adapter did.
        #[test]
        fn around_linux_the_proof_carries_what_the_inner_adapter_withheld() {
            let entries = vec![
                super::super::super::FactWithheld {
                    fact: Fact::ExecSubpaths,
                    cause: "file-system-unlisted",
                    address: "config.workspace_root",
                    path: "/data/workspaces".to_owned(),
                    file_system: Some("xfs".to_owned()),
                },
                super::super::super::FactWithheld {
                    fact: Fact::ExecSubpaths,
                    cause: "mount-id-unreported",
                    address: "config.exec_gate",
                    path: "/usr".to_owned(),
                    file_system: None,
                },
            ];
            let mut inner = AdapterFacts::new(
                AdapterName::Linux,
                None,
                &[(ContractFamily::ProcessRun, true)],
                &[(Fact::StdinPipe, true)],
            );
            inner.withheld.clone_from(&entries);
            let facts = proof(InnerAdapter::Linux, &inner, &[], &[]);
            assert_eq!(facts.adapter, AdapterName::Systemd);
            assert_eq!(facts.withheld, entries);
            assert!(!facts.proves(&Fact::ExecSubpaths));
            // An inner proof with no entry gives a proof with none.
            inner.withheld.clear();
            assert!(
                proof(InnerAdapter::Linux, &inner, &[], &[])
                    .withheld
                    .is_empty()
            );
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

        /// Every address form but one `unix:path=` is refused by `reach` before it runs a tool.
        #[test]
        fn a_session_bus_other_than_one_unix_path_is_unserved() {
            assert!(is_unix_path_address(std::ffi::OsStr::new(
                "unix:path=/run/user/1000/bus"
            )));
            assert!(is_unix_path_address(std::ffi::OsStr::new(
                "unix:path=/run/user/1000/bus,guid=0123"
            )));
            for bus in [
                "unixexec:path=/usr/bin/touch,argv1=/x",
                "unix:path=/run/user/1000/bus;unixexec:path=/usr/bin/touch",
                "unix:abstract=/tmp/dbus-x",
                "unix:path=",
                "tcp:host=localhost,port=1",
                "launchd:env=X",
                "autolaunch:",
                "/run/user/1000/bus",
            ] {
                assert!(!is_unix_path_address(std::ffi::OsStr::new(bus)), "{bus}");
                let error = Manager::reach(Some(bus.into())).err().expect("unserved");
                assert_eq!(
                    error.code(),
                    "confinement.adapter-unserved",
                    "{bus}: {error}"
                );
            }
        }

        /// The nearest bound up the ancestry, `max` all the way is none, and a tree that carries
        /// the file nowhere cannot be read.
        #[test]
        fn the_enclosing_bound_is_the_nearest_one_up_the_cgroup_ancestry() {
            let root = tempfile::tempdir().expect("root");
            let slice = root.path().join("user.slice");
            let unit = slice.join("bounded.scope");
            let leaf = unit.join("leaf");
            std::fs::create_dir_all(&leaf).expect("tree");
            std::fs::write(slice.join("pids.max"), "169092\n").expect("slice");
            std::fs::write(unit.join("pids.max"), "4242\n").expect("unit");
            std::fs::write(leaf.join("pids.max"), "max\n").expect("leaf");
            std::fs::write(slice.join("memory.max"), "max\n").expect("slice");
            assert_eq!(
                enclosing_bound(root.path(), &leaf, "pids.max"),
                Ok(Some(4242))
            );
            assert_eq!(enclosing_bound(root.path(), &leaf, "memory.max"), Ok(None));
            assert!(enclosing_bound(root.path(), &leaf, "io.max").is_err());
            std::fs::write(unit.join("pids.max"), "lots\n").expect("unit");
            assert!(enclosing_bound(root.path(), &leaf, "pids.max").is_err());
            // Nothing at or above the root is read.
            std::fs::write(root.path().join("io.max"), "1\n").expect("root");
            assert!(enclosing_bound(root.path(), &leaf, "io.max").is_err());
        }
    }

    /// Design 36, story 1, on `systemd` around `local`: the limit a run names is one the watch
    /// imposed to end it, recorded before its kill (D7), in D4's order (`EndRead::of`). A
    /// directory written as files stands for the run's cgroup; no case needs a delegated scope.
    #[cfg(test)]
    mod limit_tests {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};

        use super::*;

        /// A run's cgroup as files: `cpu.stat` at `usage_usec`. The watch's kill writes
        /// `cgroup.kill` as a plain file.
        fn cgroup_at(usage_usec: u64) -> tempfile::TempDir {
            let cgroup = tempfile::tempdir().expect("cgroup");
            std::fs::write(
                cgroup.path().join("cpu.stat"),
                format!("usage_usec {usage_usec}\n"),
            )
            .expect("cpu.stat");
            cgroup
        }

        /// Rule: a run whose leader exited on its own before its deadline names no limit, also
        /// when the deadline passes while the end task still drains the tree and has not
        /// published the end. The leader exits 100 ms into a 200 ms deadline; the end is read
        /// and published 250 ms later.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_run_whose_leader_exited_before_its_deadline_names_no_limit_while_its_tree_drains()
         {
            let cgroup = cgroup_at(0);
            // Never signalled: by the time the watch fires the leader reads exited.
            let leader = Leader::new(i32::MAX);
            let (phase, observed) = watch::channel(Phase::Running);
            let imposed = Arc::new(Mutex::new(None));
            SystemdLocal::watch(
                cgroup.path().to_path_buf(),
                leader.clone(),
                ProcessBounds {
                    timeout_ms: 200,
                    processes: 8,
                    memory_bytes: 64 << 20,
                    cpu_millis: 60_000,
                },
                observed,
                None,
                Arc::clone(&imposed),
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
            // What the end task does once `await_exit_unreaped` returned.
            *leader.exited.lock() = true;
            // The drain outlasts the deadline; then the end task reads `imposed` and publishes.
            tokio::time::sleep(Duration::from_millis(250)).await;
            let end = EndRead::of(None, Some(0), *imposed.lock());
            let _ = phase.send(Phase::at_end(
                Some(std::process::ExitStatus::from_raw(0)),
                end,
            ));
            assert_eq!(
                end.limit, None,
                "a run whose leader exited 0 before its deadline names a limit"
            );
        }

        /// Rule: a run the embedder's `KILL` ended names no limit, also when its deadline passes
        /// before the end task has read the end. The embedder stops the run; the watch's 1 ms
        /// deadline passes 100 ms before the end is read.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_run_the_embedder_stopped_names_no_limit_when_its_deadline_passes_after() {
            let cgroup = cgroup_at(0);
            // No such process group: the stop's signals reach nothing.
            let leader = Leader::new(i32::MAX);
            leader.stop(cgroup.path()).expect("cgroup.kill written");
            let (_phase, observed) = watch::channel(Phase::Running);
            let imposed = Arc::new(Mutex::new(None));
            SystemdLocal::watch(
                cgroup.path().to_path_buf(),
                leader,
                ProcessBounds {
                    timeout_ms: 1,
                    processes: 8,
                    memory_bytes: 64 << 20,
                    cpu_millis: 60_000,
                },
                observed,
                None,
                Arc::clone(&imposed),
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(
                EndRead::of(None, Some(0), *imposed.lock()).limit,
                None,
                "a run the embedder ended names a limit"
            );
        }

        /// Rule (D1, D5): a stall's kill that takes the leader's lock after the embedder's `KILL`
        /// answers that it ended nothing, so the run names no `OutputStall`. The stall closed the
        /// queue; the embedder's `KILL` then ran in full (`signal`: its `close` finds the queue
        /// closed, then `Leader::stop`); then the stall's kill takes the lock. The stand-in pid
        /// names no child, so `exited_unreaped` answers `false`, as it does for a leader whose
        /// `SIGKILL` the kernel has not acted on yet.
        #[test]
        fn a_stalls_kill_after_the_embedders_kill_answers_that_it_ended_nothing() {
            let cgroup = cgroup_at(0);
            let leader = Leader::new(i32::MAX);
            leader.stop(cgroup.path()).expect("cgroup.kill written");
            assert!(
                !leader.end_stalled(cgroup.path()),
                "a stall's kill after the embedder stopped the run answers that it ended the run, \
                 so the run names `OutputStall`"
            );
        }

        /// Rule (D4, D7): one tick in which the CPU budget and the deadline are both passed
        /// names `CpuTime`, and the record is written before the kill reaches the run.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_tick_past_both_the_cpu_budget_and_the_deadline_records_cpu_time_before_the_kill()
        {
            let cgroup = cgroup_at(5_000_000);
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "exec sleep 30"])
                .process_group(0)
                .spawn()
                .expect("a leader");
            let leader = Leader::new(i32::try_from(child.id()).expect("pid"));
            let (_phase, observed) = watch::channel(Phase::Running);
            let imposed = Arc::new(Mutex::new(None));
            SystemdLocal::watch(
                cgroup.path().to_path_buf(),
                leader,
                ProcessBounds {
                    timeout_ms: 1,
                    processes: 8,
                    memory_bytes: 64 << 20,
                    cpu_millis: 1,
                },
                observed,
                None,
                Arc::clone(&imposed),
            );
            let status = tokio::task::spawn_blocking(move || child.wait())
                .await
                .expect("joined")
                .expect("waited");
            assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
            assert_eq!(*imposed.lock(), Some(RunLimit::CpuTime));
            assert_eq!(
                std::fs::read_to_string(cgroup.path().join("cgroup.kill")).expect("killed"),
                "1"
            );
        }

        /// Rule (D4, D8): of an OOM kill in the run's cgroup and the bound the watch imposed,
        /// memory is named; with no OOM kill the imposed bound; an unreadable counter is absent,
        /// never zero, and names no memory limit.
        #[test]
        fn the_end_read_names_memory_before_the_imposed_bound_and_keeps_an_unread_count_absent() {
            for (kills, imposed, limit) in [
                (Some(1), Some(RunLimit::WallTime), Some(RunLimit::Memory)),
                (Some(2), Some(RunLimit::CpuTime), Some(RunLimit::Memory)),
                (Some(1), None, Some(RunLimit::Memory)),
                (Some(0), Some(RunLimit::CpuTime), Some(RunLimit::CpuTime)),
                (Some(0), Some(RunLimit::WallTime), Some(RunLimit::WallTime)),
                (None, Some(RunLimit::WallTime), Some(RunLimit::WallTime)),
                (Some(0), None, None),
                (None, None, None),
            ] {
                let end = EndRead::of(None, kills, imposed);
                assert_eq!(end.limit, limit, "{kills:?} {imposed:?}");
                assert_eq!(end.memory_oom_kills, kills, "{kills:?} {imposed:?}");
            }
        }

        /// Rule (D8): the end-of-run count reads `oom_kill` and never `oom_group_kill`; a file
        /// without it, an unparsable value and a removed cgroup are absent, never zero.
        #[test]
        fn the_run_cgroups_oom_count_is_its_own_oom_kill_and_absent_where_unreadable() {
            let cgroup = tempfile::tempdir().expect("cgroup");
            let events = cgroup.path().join("memory.events");
            std::fs::write(&events, "oom_group_kill 7\noom 3\noom_kill 2\n").expect("events");
            assert_eq!(oom_kills(cgroup.path()), Some(2));
            std::fs::write(&events, "oom_group_kill 7\noom 3\n").expect("events");
            assert_eq!(oom_kills(cgroup.path()), None);
            std::fs::write(&events, "oom_kill many\n").expect("events");
            assert_eq!(oom_kills(cgroup.path()), None);
            std::fs::remove_file(&events).expect("removed");
            assert_eq!(oom_kills(cgroup.path()), None);
            assert_eq!(oom_kills(&cgroup.path().join("gone")), None);
        }
    }
}
