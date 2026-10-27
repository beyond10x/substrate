//! The `macos` adapter: a host process under a Seatbelt profile (ADR 0037, ADR 0109 S1).
//!
//! Each start renders a [`SeatbeltPolicy`] whose only writable location is the workspace, whose
//! read-only locations are the declared roots, and whose only executable is the program itself,
//! and runs the program through `/usr/bin/sandbox-exec` with the run's environment and nothing
//! else. A `DYLD_` variable in that environment is refused `exec.environment-refused`: it would
//! steer the dynamic loader of every program the run starts.
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
//!   and mappable as code. A relative entry is beneath the workspace (`target`), refused
//!   `exec.sandbox-path-unrepresentable` when a part of it is a symbolic link; an absolute one
//!   must lie in the workspace, a declared read-only root, or a system or developer directory
//!   ([`seatbelt::SYSTEM_EXEC_SUBPATHS`], [`seatbelt::developer_root`]), else it is refused
//!   `exec.sandbox-exec-subpath-undeclared`. A policy with an exec subpath may read
//!   [`seatbelt::XCODE_SELECT_LINK`], [`seatbelt::XCODE_LICENSE_PREFERENCES`] and
//!   [`seatbelt::BUILD_READ_SUBPATHS`].
//!
//!   Inside the policy the `/usr/bin/cc` and `xcrun` shims cannot find the compiler or the SDK:
//!   they ask `xcodebuild` through `/bin/sh`, which no exec subpath allows. So `open` asks
//!   `xcrun`, unconfined and with a cleared environment, for the selected developer tools' macOS
//!   SDK, `clang` and `ar`, and a start gets `SDKROOT`, `CARGO_TARGET_<TRIPLE>_LINKER` for both
//!   Apple triples, `CC` and `AR` for each of them that lies beneath one of its exec subpaths,
//!   unless the command's environment names it
//!   (`story:confined-cargo-build-links-without-the-xcrun-shim`). The linker variable outranks a
//!   linker a cargo configuration file names; a caller that wants another one sets the variable.
//!   A start whose exec subpaths reach the developer tools is refused
//!   `exec.sandbox-developer-tools-unresolved`, naming why, and nothing runs, when the open could
//!   not resolve them, when another developer root is selected than at open, when the SDK the open
//!   resolved is gone, or when its exec subpaths hold the selected developer directory but not a
//!   value it needs. Nothing is resolved again at a start. The policy itself gains no rule.
//! - `private_tmpdir`: each start creates a directory beside the host's temporary directory,
//!   writable by that run alone, sets `TMPDIR` to it, and removes it before `wait` returns, after
//!   making every directory the run left in it owner-accessible again. A directory that still
//!   cannot be removed is a `private_tmpdir_left` audit record and a warning, never silence. A
//!   render cannot name a directory only a start creates, so it is `confinement.render-unserved`.
//!
//! `open` fails closed: off macOS, or when [`seatbelt::probe`] does not show the sandbox enforcing
//! a deny-default profile, it answers `confinement.adapter-unserved`. Nothing falls back to an
//! unconfined run.

use std::collections::BTreeMap;
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
    Ok(Arc::new(MacosAdapter {
        developer: DeveloperTools::resolve(),
        ..MacosAdapter::with_facts(proved(seatbelt::probe_build()))
    }))
}

/// The Apple targets the developer tools' `clang` links for. Cargo reads the linker of the
/// toolchain's host or `--target` triple, which need not be the architecture this adapter was
/// built for (an `x86_64` toolchain on Apple silicon, a daemon under Rosetta), so each is named.
const APPLE_DARWIN_TRIPLES: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

/// The environment `/usr/bin/xcrun` runs in at open: nothing of the embedding process's, so no
/// `DEVELOPER_DIR` or `TOOLCHAINS` there chooses which developer tools answer, as the Seatbelt
/// probes clear theirs.
const XCRUN_PATH: &str = "/usr/bin:/bin";

/// The selected developer tools as a build links with them, resolved outside the sandbox at open
/// (`story:confined-cargo-build-links-without-the-xcrun-shim`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeveloperTools {
    /// The host had no developer tools at open.
    Absent,
    /// The macOS SDK as `xcrun` named it, and the resolved toolchain `clang` and `ar`, each
    /// resolving beneath `root`, the developer root [`seatbelt::developer_root`] named at open.
    Resolved {
        root: PathBuf,
        sdk: PathBuf,
        clang: PathBuf,
        ar: PathBuf,
    },
    /// The host had developer tools at `root`, and `detail` says why the SDK, `clang` or `ar` was
    /// not resolved.
    Unresolved { root: PathBuf, detail: String },
}

impl DeveloperTools {
    /// Asks `/usr/bin/xcrun`, unconfined, with a cleared environment and `/` as its working
    /// directory, for the SDK, `clang` and `ar` of the selected developer tools. `xcrun` runs only
    /// when [`seatbelt::developer_root`] finds developer tools, so a host without them is never
    /// offered their installation.
    fn resolve() -> Self {
        let Some(root) = seatbelt::developer_root() else {
            return Self::Absent;
        };
        resolve_developer_tools(root, |args| {
            let output = std::process::Command::new("/usr/bin/xcrun")
                .args(["--sdk", "macosx"])
                .args(args)
                .env_clear()
                .env("PATH", XCRUN_PATH)
                .current_dir("/")
                .stdin(Stdio::null())
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "{}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            String::from_utf8(output.stdout).map_err(|_| "the answer is not UTF-8".to_owned())
        })
    }

    /// The developer root the open saw, if it saw one.
    #[cfg(test)]
    fn root(&self) -> Option<&Path> {
        match self {
            Self::Absent => None,
            Self::Resolved { root, .. } | Self::Unresolved { root, .. } => Some(root),
        }
    }

    /// [`Self::environment_at`] with the selection unchanged since the open.
    #[cfg(test)]
    fn environment(
        &self,
        exec_subpaths: &[PathBuf],
        caller: &BTreeMap<String, String>,
    ) -> Result<Vec<(String, PathBuf)>, ConfinementError> {
        self.environment_at(self.root(), exec_subpaths, caller)
    }

    /// What a start adds to its environment so a build links and compiles C with the developer
    /// tools and nothing asks `xcrun`: `SDKROOT`, `CARGO_TARGET_<TRIPLE>_LINKER` for each of
    /// [`APPLE_DARWIN_TRIPLES`], `CC` and `AR`. A start gets each value whose path lies beneath one
    /// of its exec subpaths, unless its environment names it. `selected` is the developer root
    /// [`seatbelt::developer_root`] names now; nothing is resolved again.
    ///
    /// A start whose exec subpaths hold the selected developer directory, or any value, is refused
    /// `exec.sandbox-developer-tools-unresolved`, naming why, when the open did not resolve the
    /// tools, when the selection changed since the open, when the SDK the open resolved is gone,
    /// or when its exec subpaths hold the selected developer directory but a value it needs lies
    /// outside them.
    fn environment_at(
        &self,
        selected: Option<&Path>,
        exec_subpaths: &[PathBuf],
        caller: &BTreeMap<String, String>,
    ) -> Result<Vec<(String, PathBuf)>, ConfinementError> {
        let beneath = |path: &Path| {
            exec_subpaths
                .iter()
                .any(|subpath| path.starts_with(subpath))
        };
        let holds = |root: &Path| beneath(&selected_directory(root));
        let refused = |why: String| -> ConfinementError {
            DriverError::unserved(seatbelt::DEVELOPER_TOOLS_UNRESOLVED, why, "exec_subpaths").into()
        };
        let (root, sdk, clang, ar) = match self {
            Self::Absent => {
                return match selected {
                    Some(now) if holds(now) => Err(refused(format!(
                        "the exec subpaths hold the developer tools at {}, selected after the \
                         open, which found none",
                        now.display()
                    ))),
                    _ => Ok(Vec::new()),
                };
            }
            Self::Unresolved { root, detail } => {
                return if holds(root) || selected.is_some_and(holds) {
                    Err(refused(format!(
                        "the exec subpaths hold the developer tools at {}, whose SDK, clang and \
                         ar were not resolved at open: {detail}",
                        root.display()
                    )))
                } else {
                    Ok(Vec::new())
                };
            }
            Self::Resolved {
                root,
                sdk,
                clang,
                ar,
            } => (root, sdk, clang, ar),
        };
        let sdk_now = std::fs::canonicalize(sdk)
            .ok()
            .filter(|path| path.is_dir() && path.starts_with(root));
        let involved = holds(root)
            || selected.is_some_and(holds)
            || beneath(clang)
            || beneath(ar)
            || sdk_now.as_deref().is_some_and(beneath);
        if !involved {
            return Ok(Vec::new());
        }
        if selected != Some(root.as_path()) {
            return Err(refused(format!(
                "the developer tools selected now, {}, are not the ones the open resolved, {}",
                selected.map_or_else(|| "none".to_owned(), |now| now.display().to_string()),
                root.display()
            )));
        }
        let Some(sdk_now) = sdk_now else {
            return Err(refused(format!(
                "the SDK the open resolved, {}, is no longer a directory beneath {}",
                sdk.display(),
                root.display()
            )));
        };
        // Each value: its name, the resolved path the policy must reach, and the value itself.
        let mut values = vec![("SDKROOT".to_owned(), sdk_now, sdk.clone())];
        for triple in APPLE_DARWIN_TRIPLES {
            values.push((linker_variable(triple), clang.clone(), clang.clone()));
        }
        values.push(("CC".to_owned(), clang.clone(), clang.clone()));
        values.push(("AR".to_owned(), ar.clone(), ar.clone()));
        values.retain(|(name, _, _)| !caller.contains_key(name));
        if holds(root)
            && let Some((name, reached, _)) =
                values.iter().find(|(_, reached, _)| !beneath(reached))
        {
            return Err(refused(format!(
                "the exec subpaths hold the selected developer directory {} but not {name}, {}",
                selected_directory(root).display(),
                reached.display()
            )));
        }
        Ok(values
            .into_iter()
            .filter(|(_, reached, _)| beneath(reached))
            .map(|(name, _, value)| (name, value))
            .collect())
    }
}

/// The developer directory `xcode-select` selects for the developer root `root`: an Xcode
/// bundle's `Contents/Developer`, else the root itself (the Command Line Tools). The inverse of
/// [`seatbelt::developer_root`].
fn selected_directory(root: &Path) -> PathBuf {
    if root.extension().is_some_and(|extension| extension == "app") {
        root.join("Contents/Developer")
    } else {
        root.to_path_buf()
    }
}

/// The developer tools at `root` as `xcrun` names them: `xcrun` answers `--sdk macosx <args>` for
/// `--show-sdk-path`, `--find clang` and `--find ar`. Each answer must be an absolute path that
/// resolves beneath `root`. The SDK is kept as `xcrun` named it, so an SDK update that moves the
/// unversioned link keeps it valid; `clang` and `ar` are kept resolved.
fn resolve_developer_tools(
    root: PathBuf,
    xcrun: impl Fn(&[&str]) -> Result<String, String>,
) -> DeveloperTools {
    let find = |args: &[&str]| -> Result<(PathBuf, PathBuf), String> {
        let asked = format!("xcrun --sdk macosx {}", args.join(" "));
        let answer = xcrun(args).map_err(|error| format!("{asked}: {error}"))?;
        let answer = answer.trim();
        if answer.is_empty() {
            return Err(format!("{asked} named no path"));
        }
        if !Path::new(answer).is_absolute() {
            return Err(format!("{asked} named {answer}, not an absolute path"));
        }
        let path = std::fs::canonicalize(answer)
            .map_err(|error| format!("{asked} named {answer}: {error}"))?;
        if !path.starts_with(&root) {
            return Err(format!(
                "{asked} named {}, outside the developer tools {}",
                path.display(),
                root.display()
            ));
        }
        Ok((PathBuf::from(answer), path))
    };
    let resolved = find(&["--show-sdk-path"]).and_then(|(sdk, _)| {
        let (_, clang) = find(&["--find", "clang"])?;
        let (_, ar) = find(&["--find", "ar"])?;
        Ok((sdk, clang, ar))
    });
    match resolved {
        Ok((sdk, clang, ar)) => DeveloperTools::Resolved {
            root,
            sdk,
            clang,
            ar,
        },
        Err(detail) => DeveloperTools::Unresolved { root, detail },
    }
}

/// The Apple target triple of the architecture this adapter was built for.
#[cfg(test)]
fn apple_host_triple() -> String {
    format!("{}-apple-darwin", std::env::consts::ARCH)
}

/// The variable cargo reads `triple`'s linker from: `CARGO_TARGET_<TRIPLE>_LINKER`, the triple
/// upper-cased with `-` and `.` as `_`.
fn linker_variable(triple: &str) -> String {
    format!(
        "CARGO_TARGET_{}_LINKER",
        triple.to_ascii_uppercase().replace(['-', '.'], "_")
    )
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
    developer: DeveloperTools,
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
            developer: DeveloperTools::Absent,
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

    /// The policy of [`Self::policy_and_exec_subpaths`] alone.
    #[cfg(test)]
    fn policy(
        admitted: &AdmittedProfile,
        program: &Path,
        private_tmpdir: Option<&Path>,
    ) -> Result<SeatbeltPolicy, ConfinementError> {
        Self::policy_and_exec_subpaths(admitted, program, private_tmpdir).map(|(policy, _)| policy)
    }

    /// The Seatbelt policy one start of `program` runs under: the workspace, the declared roots,
    /// the private `TMPDIR` when the start created one, the sealed paths and the exec subpaths,
    /// each after the refusals a start applies to it; and those exec subpaths as the policy names
    /// them.
    fn policy_and_exec_subpaths(
        admitted: &AdmittedProfile,
        program: &Path,
        private_tmpdir: Option<&Path>,
    ) -> Result<(SeatbeltPolicy, Vec<PathBuf>), ConfinementError> {
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
        let mut subpaths = Vec::with_capacity(profile.exec_subpaths.len());
        for subpath in &profile.exec_subpaths {
            let subpath = exec_subpath(&workspace, &declared, subpath)?;
            subpaths.push(subpath.clone());
            policy = policy.exec_subpath(subpath);
        }
        Ok((policy, subpaths))
    }

    /// The confined command one start would spawn: the Seatbelt launcher, its profile, the
    /// program and its arguments, in the workspace or the workdir beneath it, with `TMPDIR` set to
    /// the private directory when there is one, and the developer tools' values the exec subpaths
    /// reach ([`DeveloperTools::environment_at`]). Nothing is spawned.
    fn command(
        developer: &DeveloperTools,
        admitted: &AdmittedProfile,
        command: &Command,
        private_tmpdir: Option<&Path>,
    ) -> Result<std::process::Command, ConfinementError> {
        super::check_command(command)?;
        crate::environment::check(&command.env)?;
        // A `DYLD_` variable steers the dynamic loader of every program the run starts: a library
        // inserted into it, a search path ahead of the system's. None reaches a confined run.
        if command.env.keys().any(|name| name.starts_with("DYLD_")) {
            return Err(crate::environment::refused().into());
        }
        let program = Path::new(&command.argv[0]);
        if !program.is_absolute() {
            return Err(DriverError::refused(
                seatbelt::PATH_UNREPRESENTABLE,
                "The program must be an absolute path: a confined run has no search path.",
                "argv",
            )
            .into());
        }
        let (policy, exec_subpaths) =
            Self::policy_and_exec_subpaths(admitted, program, private_tmpdir)?;
        let developer = developer.environment_at(
            seatbelt::developer_root().as_deref(),
            &exec_subpaths,
            &command.env,
        )?;
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
        // Names the caller did not set, so nothing the caller named is replaced.
        confined.envs(developer);
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
    beneath_without_symlink(workspace, path, "sealed path", "sealed_paths")
}

/// `path` beneath the resolved workspace, refused `exec.sandbox-path-unrepresentable` when it is
/// not a normal relative path or when any part of it that exists is a symbolic link.
fn beneath_without_symlink(
    workspace: &Path,
    path: &str,
    what: &str,
    field: &str,
) -> Result<PathBuf, ConfinementError> {
    seatbelt::relative_literal(path)?;
    let mut current = workspace.to_path_buf();
    for name in path.split('/') {
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(DriverError::refused(
                    seatbelt::PATH_UNREPRESENTABLE,
                    format!(
                        "the {what} {path} is reached through the symbolic link {}",
                        current.display()
                    ),
                    field,
                )
                .into());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(workspace.join(path))
}

/// An exec subpath, resolved: a relative one beneath the workspace (it need not exist yet, and
/// no part of it that exists may be a symbolic link, which would carry the rule out of the
/// workspace), an absolute one only inside the workspace, a declared root, or a system or
/// developer directory.
fn exec_subpath(
    workspace: &Path,
    declared: &[PathBuf],
    subpath: &str,
) -> Result<PathBuf, ConfinementError> {
    if !subpath.starts_with('/') {
        return beneath_without_symlink(workspace, subpath, "exec subpath", "exec_subpaths");
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

/// Removes a run's private `TMPDIR` once the run has ended. What the run left there may refuse
/// its owner (a directory `chmod 000`), so every directory is first made owner-accessible; one
/// that still cannot be removed (a user immutable flag, for one) is audited and logged by name.
fn remove_private_tmpdir(admitted: &AdmittedProfile, run: &RunId, path: &Path) {
    make_removable(path);
    if let Err(error) = std::fs::remove_dir_all(path) {
        tracing::warn!(
            run = run.as_str(),
            path = %path.display(),
            %error,
            "the macos private TMPDIR could not be removed after the run"
        );
        admitted.audit_private_tmpdir_left(run, path, &error);
    }
}

/// Gives the owner read, write and search on every directory beneath `path`, never through a
/// symbolic link. Best effort: the removal that follows decides.
fn make_removable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut pending = vec![path.to_path_buf()];
    while let Some(entry) = pending.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let mode = metadata.permissions().mode() | 0o700;
        let _ = std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(mode));
        if let Ok(children) = std::fs::read_dir(&entry) {
            pending.extend(children.flatten().map(|child| child.path()));
        }
    }
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
            &self.developer,
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
        let ending = admitted.clone();
        let ending_id = id.clone();
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
            // Removed before the run is observed ended, so `wait` never returns while it exists
            // unless its removal failed, which is audited.
            if let Some((directory, resolved)) = private {
                // The removal below decides; the guard must not try again, silently.
                let _ = directory.keep();
                remove_private_tmpdir(&ending, &ending_id, &resolved);
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
        let confined = Self::command(&self.developer, admitted, command, None)?;
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

    // Security F1: the relative entry would otherwise resolve through the link and make its target
    // readable and executable.
    #[test]
    fn a_relative_exec_subpath_reached_through_a_symlink_is_refused() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir(outside.path().join("bin")).expect("bin");
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("target")).expect("link");
        for subpath in ["target", "target/bin", "target/absent"] {
            let admitted = admitted_in(
                workspace.path(),
                ConfinementProfile {
                    exec_subpaths: vec![subpath.into()],
                    ..ConfinementProfile::default()
                },
            );
            let error =
                MacosAdapter::policy(&admitted, Path::new("/bin/sh"), None).expect_err(subpath);
            assert_eq!(
                driver_code(&error),
                seatbelt::PATH_UNREPRESENTABLE,
                "{subpath}"
            );
        }
        let admitted = admitted_in(
            workspace.path(),
            ConfinementProfile {
                exec_subpaths: vec!["built/later".into()],
                ..ConfinementProfile::default()
            },
        );
        MacosAdapter::policy(&admitted, Path::new("/bin/sh"), None)
            .expect("a relative exec subpath not created yet");
    }

    /// A directory the owner cannot read, write or search, holding a file.
    fn locked_tree(parent: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let private = parent.join("private");
        let kept = private.join("kept");
        std::fs::create_dir_all(&kept).expect("kept");
        std::fs::write(kept.join("file"), b"x").expect("file");
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o000)).expect("lock");
        private
    }

    // Security F2: what the run left in its TMPDIR does not keep the TMPDIR.
    #[test]
    fn a_private_tmpdir_holding_a_locked_directory_is_removed() {
        let parent = tempfile::tempdir().expect("parent");
        let private = locked_tree(parent.path());
        let (sink, records) = recording();
        let admitted = admitted_in(
            parent.path(),
            ConfinementProfile {
                audit: Some(sink),
                ..ConfinementProfile::default()
            },
        );
        let before = records.lock().len();
        remove_private_tmpdir(&admitted, &RunId::new("macos-1-1".to_owned()), &private);
        assert!(!private.exists(), "the private TMPDIR outlived its removal");
        assert_eq!(records.lock().len(), before, "a removal was audited");
    }

    // Security F2: a TMPDIR that cannot be removed is audited by name, never reported gone.
    #[test]
    fn a_private_tmpdir_that_cannot_be_removed_is_audited() {
        use std::os::unix::fs::PermissionsExt as _;
        let parent = tempfile::tempdir().expect("parent");
        let holder = parent.path().join("holder");
        std::fs::create_dir(&holder).expect("holder");
        let private = locked_tree(&holder);
        std::fs::set_permissions(&holder, std::fs::Permissions::from_mode(0o500)).expect("hold");
        let (sink, records) = recording();
        let admitted = admitted_in(
            parent.path(),
            ConfinementProfile {
                audit: Some(sink),
                ..ConfinementProfile::default()
            },
        );
        let before = records.lock().len();
        remove_private_tmpdir(&admitted, &RunId::new("macos-1-1".to_owned()), &private);
        let left = private.exists();
        std::fs::set_permissions(&holder, std::fs::Permissions::from_mode(0o700)).expect("free");
        let kept = private.join("kept");
        if kept.exists() {
            std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o700))
                .expect("unlock");
        }
        assert!(left, "the parent did not hold the private TMPDIR");
        let records = records.lock();
        assert_eq!(records.len(), before + 1, "{records:?}");
        let record = serde_json::to_value(&records[before]).expect("record");
        assert_eq!(record["event"], "private_tmpdir_left", "{record}");
        assert_eq!(record["run"], "macos-1-1", "{record}");
        assert_eq!(record["path"], private.to_str().expect("UTF-8"), "{record}");
        assert!(
            record["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "{record}"
        );
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

    // story:confined-cargo-build-links-without-the-xcrun-shim: cargo reads the host target's
    // linker from `CARGO_TARGET_<TRIPLE>_LINKER`, the triple upper-cased with `-` as `_`.
    #[test]
    fn the_linker_variable_names_the_host_target() {
        assert_eq!(
            linker_variable("aarch64-apple-darwin"),
            "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER"
        );
        assert_eq!(
            linker_variable("x86_64-apple-darwin"),
            "CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER"
        );
        assert_eq!(
            apple_host_triple(),
            format!("{}-apple-darwin", std::env::consts::ARCH)
        );
    }

    /// A developer root holding an SDK directory and `clang` and `ar` files where `xcrun` names
    /// them, and a directory beside the root holding its own `clang` and `ar`.
    struct DeveloperTree {
        _root: tempfile::TempDir,
        developer: PathBuf,
        sdk: PathBuf,
        clang: PathBuf,
        ar: PathBuf,
        beside: PathBuf,
    }

    fn developer_tree() -> DeveloperTree {
        let root = tempfile::tempdir().expect("root");
        let base = std::fs::canonicalize(root.path()).expect("canonical");
        let developer = base.join("Xcode.app");
        let sdk = developer.join("Contents/Developer/SDKs/MacOSX.sdk");
        let bin = developer.join("Contents/Developer/Toolchains/Default.xctoolchain/usr/bin");
        let beside = base.join("beside");
        for directory in [&sdk, &bin, &beside] {
            std::fs::create_dir_all(directory).expect("directory");
        }
        for tool in ["clang", "ar"] {
            std::fs::write(bin.join(tool), b"").expect(tool);
            std::fs::write(beside.join(tool), b"").expect(tool);
        }
        DeveloperTree {
            _root: root,
            developer,
            sdk,
            clang: bin.join("clang"),
            ar: bin.join("ar"),
            beside,
        }
    }

    /// What `xcrun --sdk macosx <args>` answers: `sdk` for the SDK path, `clang` for the linker,
    /// and for `ar` the `ar` beside the `clang` it names.
    fn answering(
        sdk: Result<String, String>,
        clang: Result<String, String>,
    ) -> impl Fn(&[&str]) -> Result<String, String> {
        move |args: &[&str]| match args {
            ["--show-sdk-path"] => sdk.clone(),
            ["--find", "clang"] => clang.clone(),
            ["--find", "ar"] => clang
                .clone()
                .map(|clang| clang.replace("/clang\n", "/ar\n")),
            other => panic!("unexpected xcrun arguments {other:?}"),
        }
    }

    /// `xcrun` naming `path`, as it prints it.
    fn line(path: &Path) -> String {
        format!("{}\n", path.display())
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim: the SDK and `clang` are resolved
    // outside the sandbox, and only a pair the developer root holds is used, because a build
    // policy reaches the developer tools through that root and nothing else.
    #[test]
    fn the_developer_tools_resolve_only_beneath_their_root() {
        let tree = developer_tree();
        let resolved = resolve_developer_tools(
            tree.developer.clone(),
            answering(Ok(line(&tree.sdk)), Ok(line(&tree.clang))),
        );
        assert_eq!(
            resolved,
            DeveloperTools::Resolved {
                root: tree.developer.clone(),
                sdk: tree.sdk.clone(),
                clang: tree.clang.clone(),
                ar: tree.ar.clone(),
            }
        );
        for (sdk, clang, named) in [
            (
                Ok("relative/MacOSX.sdk\n".to_owned()),
                Ok(line(&tree.clang)),
                "not an absolute path",
            ),
            (Ok(line(&tree.beside)), Ok(line(&tree.clang)), "beside"),
            (
                Ok(line(&tree.sdk)),
                Ok(line(&tree.beside.join("clang"))),
                "beside",
            ),
            (Err("exit 72".to_owned()), Ok(line(&tree.clang)), "exit 72"),
            (Ok(line(&tree.sdk)), Err("exit 72".to_owned()), "exit 72"),
            (
                Ok(line(&tree.sdk.join("absent"))),
                Ok(line(&tree.clang)),
                "absent",
            ),
            (
                Ok("\n".to_owned()),
                Ok(line(&tree.clang)),
                "--show-sdk-path",
            ),
        ] {
            match resolve_developer_tools(tree.developer.clone(), answering(sdk, clang)) {
                DeveloperTools::Unresolved { root, detail } => {
                    assert_eq!(root, tree.developer);
                    assert!(detail.contains(named), "{named}: {detail}");
                }
                other => panic!("{named}: expected unresolved tools, got {other:?}"),
            }
        }
    }

    fn resolved(tree: &DeveloperTree) -> DeveloperTools {
        DeveloperTools::Resolved {
            root: tree.developer.clone(),
            sdk: tree.sdk.clone(),
            clang: tree.clang.clone(),
            ar: tree.ar.clone(),
        }
    }

    /// Every value a start over the whole tree gets, in the order the adapter names them.
    fn every_value(tree: &DeveloperTree) -> Vec<(String, PathBuf)> {
        vec![
            ("SDKROOT".to_owned(), tree.sdk.clone()),
            (
                "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER".to_owned(),
                tree.clang.clone(),
            ),
            (
                "CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER".to_owned(),
                tree.clang.clone(),
            ),
            ("CC".to_owned(), tree.clang.clone()),
            ("AR".to_owned(), tree.ar.clone()),
        ]
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim: an exec subpath that holds the
    // developer root gets `SDKROOT`, the linker for both Apple triples, `CC` and `AR`, so cargo
    // and a build script never reach the `/usr/bin/cc` shim; each value is given only to a policy
    // that reaches it (adversary A2), so one over the toolchain alone gets the toolchain's
    // values, and a policy that reaches none of the developer tools gets nothing.
    #[test]
    fn a_build_policy_over_the_developer_tools_gets_sdkroot_and_the_linker() {
        let tree = developer_tree();
        let tools = resolved(&tree);
        let parent = tree.developer.parent().expect("parent").to_path_buf();
        for subpaths in [
            vec![tree.developer.clone()],
            vec![tree.developer.join("Contents/Developer")],
            vec![PathBuf::from("/usr/bin"), parent],
        ] {
            assert_eq!(
                tools
                    .environment(&subpaths, &BTreeMap::new())
                    .expect("environment"),
                every_value(&tree),
                "{subpaths:?}"
            );
        }
        let toolchain = vec![tree.developer.join("Contents/Developer/Toolchains")];
        assert_eq!(
            tools
                .environment(&toolchain, &BTreeMap::new())
                .expect("environment"),
            every_value(&tree)[1..].to_vec(),
        );
        for subpaths in [
            vec![],
            vec![PathBuf::from("/usr/bin")],
            vec![tree.beside.clone()],
        ] {
            assert_eq!(
                tools
                    .environment(&subpaths, &BTreeMap::new())
                    .expect("environment"),
                Vec::new(),
                "{subpaths:?}"
            );
        }
        assert_eq!(
            DeveloperTools::Absent
                .environment(std::slice::from_ref(&tree.developer), &BTreeMap::new())
                .expect("environment"),
            Vec::new()
        );
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim, adversary A2: a start whose exec
    // subpaths hold the selected developer directory but not a value it needs is refused by name,
    // naming the value; a value the caller set is not needed.
    #[test]
    fn a_value_outside_a_policy_over_the_selected_directory_is_refused_by_name() {
        let tree = developer_tree();
        let tools = DeveloperTools::Resolved {
            root: tree.developer.clone(),
            sdk: tree.sdk.clone(),
            clang: tree.clang.clone(),
            ar: tree.beside.join("ar"),
        };
        let subpaths = [tree.developer.clone()];
        let error = tools
            .environment(&subpaths, &BTreeMap::new())
            .expect_err("AR outside the policy");
        assert_eq!(driver_code(&error), seatbelt::DEVELOPER_TOOLS_UNRESOLVED);
        assert!(error.to_string().contains("not AR"), "{error}");
        let caller = BTreeMap::from([("AR".to_owned(), "/caller/ar".to_owned())]);
        assert_eq!(
            tools.environment(&subpaths, &caller).expect("environment"),
            every_value(&tree)[..4].to_vec()
        );
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim, security F2: developer tools
    // selected after the open were never resolved, so a start whose exec subpaths reach them, or
    // the ones the open resolved, is refused by name; a start that reaches neither is not.
    #[test]
    fn a_selection_changed_since_the_open_refuses_a_start_over_the_developer_tools() {
        let tree = developer_tree();
        let tools = resolved(&tree);
        let other = tree.beside.join("Other.app");
        std::fs::create_dir_all(other.join("Contents/Developer")).expect("other Xcode");
        for (selected, subpaths, why) in [
            (Some(other.as_path()), vec![other.clone()], "selected now"),
            (
                Some(other.as_path()),
                vec![tree.developer.clone()],
                "selected now",
            ),
            (None, vec![tree.developer.clone()], "selected now, none"),
        ] {
            let error = tools
                .environment_at(selected, &subpaths, &BTreeMap::new())
                .expect_err(why);
            assert_eq!(driver_code(&error), seatbelt::DEVELOPER_TOOLS_UNRESOLVED);
            assert!(error.to_string().contains(why), "{why}: {error}");
        }
        assert_eq!(
            tools
                .environment_at(
                    Some(other.as_path()),
                    &[PathBuf::from("/usr/bin")],
                    &BTreeMap::new()
                )
                .expect("a start that reaches no developer tools"),
            Vec::new()
        );
        let error = DeveloperTools::Absent
            .environment_at(
                Some(other.as_path()),
                std::slice::from_ref(&other),
                &BTreeMap::new(),
            )
            .expect_err("selected after an open that found none");
        assert!(error.to_string().contains("found none"), "{error}");
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim, security F2: an SDK the open
    // resolved that is gone refuses a start over the developer tools by name.
    #[test]
    fn a_vanished_sdk_refuses_a_start_over_the_developer_tools() {
        let tree = developer_tree();
        let tools = resolved(&tree);
        std::fs::remove_dir(&tree.sdk).expect("SDK removed");
        let error = tools
            .environment(std::slice::from_ref(&tree.developer), &BTreeMap::new())
            .expect_err("a vanished SDK");
        assert_eq!(driver_code(&error), seatbelt::DEVELOPER_TOOLS_UNRESOLVED);
        assert!(
            error.to_string().contains("no longer a directory"),
            "{error}"
        );
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim: a value the caller names is never
    // replaced.
    #[test]
    fn what_the_caller_set_is_kept() {
        let tree = developer_tree();
        let tools = resolved(&tree);
        let subpaths = [tree.developer.clone()];
        let every = every_value(&tree);
        for (index, (name, _)) in every.iter().enumerate() {
            let caller = BTreeMap::from([(name.clone(), "/caller/value".to_owned())]);
            let mut expected = every.clone();
            expected.remove(index);
            assert_eq!(
                tools.environment(&subpaths, &caller).expect("environment"),
                expected,
                "{name}"
            );
        }
    }

    // story:confined-cargo-build-links-without-the-xcrun-shim: developer tools the adapter could
    // not resolve at open refuse, by name and audited, a start whose policy would run them, and
    // nothing runs; a policy that does not hold them is not affected.
    #[tokio::test]
    async fn unresolved_developer_tools_refuse_a_build_policy_over_them_before_anything_runs() {
        let tree = developer_tree();
        let developer = tree.developer.to_str().expect("UTF-8").to_owned();
        let adapter = MacosAdapter {
            developer: DeveloperTools::Unresolved {
                root: tree.developer.clone(),
                detail: "xcrun --sdk macosx --find clang: exit 72".to_owned(),
            },
            ..adapter()
        };
        let unaffected = DeveloperTools::Unresolved {
            root: tree.developer.clone(),
            detail: String::new(),
        };
        assert_eq!(
            unaffected
                .environment(std::slice::from_ref(&tree.beside), &BTreeMap::new())
                .expect("a policy without the developer tools"),
            Vec::new()
        );
        let workspace = tempfile::tempdir().expect("workspace");
        let (sink, records) = recording();
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                read_only_roots: vec![root(&developer, &developer)],
                exec_subpaths: vec![developer.clone()],
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
            .expect_err("unresolved developer tools");
        assert_eq!(driver_code(&error), seatbelt::DEVELOPER_TOOLS_UNRESOLVED);
        assert!(error.to_string().contains("exit 72"), "{error}");
        assert!(!workspace.path().join("marker").exists());
        let records = records.lock();
        let last = serde_json::to_value(records.last().expect("a record")).expect("record");
        assert_eq!(last["event"], "profile_refused", "{last}");
        assert_eq!(last["code"], seatbelt::DEVELOPER_TOOLS_UNRESOLVED, "{last}");
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

    // macos-15 run 40452966164: `DYLD_INSERT_LIBRARIES` reached the confined program
    // (`a_dyld_variable_does_not_reach_the_confined_program`). Every `DYLD_` variable steers the
    // dynamic loader, so a start naming one is refused before anything runs.
    #[tokio::test]
    async fn a_dynamic_loader_variable_is_refused_before_anything_runs() {
        let adapter = adapter();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        for name in [
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FRAMEWORK_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "DYLD_IMAGE_SUFFIX",
        ] {
            let error = adapter
                .start(
                    &admitted,
                    &Command {
                        argv: vec!["/bin/sh".into()],
                        env: BTreeMap::from([(name.to_owned(), "/absent.dylib".to_owned())]),
                    },
                )
                .await
                .expect_err(name);
            assert_eq!(driver_code(&error), "exec.environment-refused", "{name}");
        }
        assert_eq!(
            std::fs::read_dir(workspace.path())
                .expect("workspace")
                .count(),
            0,
            "something ran in the workspace"
        );
    }

    // Adversary, story:confined-cargo-build-links-without-the-xcrun-shim, A2: the selected
    // developer directory (`xcode-select -p`, `…/Xcode.app/Contents/Developer`) is an admitted exec
    // subpath and holds both the SDK and `clang` the adapter resolved, so its policy can run them;
    // without `SDKROOT` and the linker its build reaches the `/usr/bin/cc` shim and fails.
    #[test]
    fn adversary_a_policy_over_the_selected_developer_directory_gets_sdkroot_and_the_linker() {
        let tree = developer_tree();
        let tools = resolved(&tree);
        let selected = tree.developer.join("Contents/Developer");
        assert!(tree.sdk.starts_with(&selected) && tree.clang.starts_with(&selected));
        // story:confined-cargo-build-links-without-the-xcrun-shim: containment, not equality,
        // because A1 adds `CC` and `AR` and A3 the other triple's linker to the same environment.
        let environment = tools
            .environment(std::slice::from_ref(&selected), &BTreeMap::new())
            .expect("environment");
        for expected in [
            ("SDKROOT".to_owned(), tree.sdk.clone()),
            (linker_variable(&apple_host_triple()), tree.clang.clone()),
        ] {
            assert!(
                environment.contains(&expected),
                "{expected:?} is not set: {environment:?}"
            );
        }
    }

    // Adversary, A3: cargo reads the linker of the toolchain's host or `--target` triple, which is
    // not the architecture the daemon was built for when the toolchain is the other Apple triple
    // (an x86_64 rustup toolchain on Apple silicon, a daemon under Rosetta, a `--target` build).
    // The resolved `clang` links both, so a start over the developer tools names it for both.
    #[test]
    fn adversary_the_linker_is_named_for_each_apple_darwin_triple() {
        let tree = developer_tree();
        let environment = resolved(&tree)
            .environment(std::slice::from_ref(&tree.developer), &BTreeMap::new())
            .expect("environment");
        for triple in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
            let name = linker_variable(triple);
            assert!(
                environment
                    .iter()
                    .any(|(set, value)| *set == name && *value == tree.clang),
                "{name} is not set: {environment:?}"
            );
        }
    }

    // Adversary, A4: `xcrun --show-sdk-path` names the unversioned `MacOSX.sdk` link, and the
    // adapter keeps its canonical, versioned target from open for the daemon's lifetime. An SDK
    // update while the daemon runs replaces the versioned directory and moves the link, and every
    // later start gets an `SDKROOT` that no longer exists; rustc ignores it, asks `xcrun`, and the
    // build fails inside the sandbox with no named refusal.
    #[test]
    fn adversary_the_sdkroot_a_start_gets_survives_an_sdk_update() {
        let tree = developer_tree();
        let sdks = tree
            .developer
            .join("Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs");
        let old = sdks.join("MacOSX15.0.sdk");
        std::fs::create_dir_all(&old).expect("versioned SDK");
        let link = sdks.join("MacOSX.sdk");
        std::os::unix::fs::symlink("MacOSX15.0.sdk", &link).expect("SDK link");
        let tools = resolve_developer_tools(
            tree.developer.clone(),
            answering(Ok(line(&link)), Ok(line(&tree.clang))),
        );
        assert!(
            matches!(tools, DeveloperTools::Resolved { .. }),
            "{tools:?}"
        );
        // The update: the versioned SDK is replaced and the link names its successor.
        std::fs::remove_dir(&old).expect("old SDK removed");
        std::fs::create_dir(sdks.join("MacOSX15.2.sdk")).expect("new SDK");
        std::fs::remove_file(&link).expect("link removed");
        std::os::unix::fs::symlink("MacOSX15.2.sdk", &link).expect("link moved");
        let environment = tools
            .environment(std::slice::from_ref(&tree.developer), &BTreeMap::new())
            .expect("environment");
        let (_, sdkroot) = environment
            .iter()
            .find(|(name, _)| name == "SDKROOT")
            .expect("SDKROOT is set");
        assert!(
            sdkroot.is_dir(),
            "SDKROOT {} no longer exists after the update",
            sdkroot.display()
        );
    }
}
