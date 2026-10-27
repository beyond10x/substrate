//! `b10x-substrate confine run`: the `sandbox` repository's command line over Substrate's
//! confinement adapters (ADR 0109), without a daemon.
//!
//! The verb builds one [`ConfinementProfile`] from its flags, opens the adapter through
//! [`confinement::open`] and lets that adapter admit, render or run it. It adds no confinement of
//! its own: every refusal is the adapter's named refusal, or one of the verb's own checks of its
//! flags, and every one exits 2 before anything runs. `--local <reason>` selects the
//! declared-insecure `local` adapter, and the verb says so on stderr.

mod cgroup;

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, ValueEnum};
use serde_json::{Value, json};
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AdmittedProfile, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, HostConfig, Network, ReadOnlyRoot,
    RunObservation, Signal, image_is_pinned, linux::LinuxRequest, local::LocalRequest,
    oci::OciRequest,
};
use substrate_host::{DriverError, DriverErrorClass};
use tokio::signal::unix::{SignalKind, signal};

use crate::{EXIT_FAILURE, EXIT_USAGE, Output, to_json, write_line};

use self::cgroup::PrivateCgroup;

/// The variable naming the delegated cgroup v2 root `linux` runs beneath. It is the CLI's own:
/// the daemon's `SUBSTRATE_CGROUP_ROOT` is never read here, because a host driver opened on a root
/// kills every run group it finds there. Each invocation makes a private group beneath this root
/// and opens its driver on that.
pub const CGROUP_ROOT_ENV: &str = "SUBSTRATE_CONFINE_CGROUP_ROOT";

/// `sandbox`'s baseline environment (`src/confinement.rs` at `1b0377d`): every run starts with
/// these five and nothing inherited from this process.
const BASELINE_ENV: [(&str, &str); 5] = [
    ("PATH", "/usr/bin:/bin"),
    ("HOME", "/tmp"),
    ("LANG", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
    ("TZ", "UTC"),
];

/// What `--help` says beyond the flags: the limits a user meets and cannot change here.
pub const AFTER_HELP: &str = "\
Bounds: linux and oci bound every run at 600 s of wall clock, 256 processes, 1 GiB of memory \
and 600 s of CPU, and its output at the host's output limit; no flag changes them.
Input: the command's standard input is closed; nothing is read from this terminal.
Command: the program and its arguments after `--` are required; there is no default shell.
Exit status: the command's own status, or 128 plus the signal that ended it. A refusal before \
anything runs exits 2 and a host failure 1, so a command that itself exits 1 or 2 is told apart \
only by its output; `--output json` separates them.
Environment: the command sees PATH=/usr/bin:/bin, HOME=/tmp, LANG=C.UTF-8, LC_ALL=C.UTF-8 and \
TZ=UTC, and nothing of this process's environment.
Cgroup: linux runs in a private group made beneath the delegated cgroup v2 root named by \
SUBSTRATE_CONFINE_CGROUP_ROOT, which this process must run beneath; without it linux runs \
nothing. The daemon's SUBSTRATE_CGROUP_ROOT is not read.
Workspace: under linux the --dir directory's name must match [A-Za-z0-9_-]+, the host's \
workspace identity rule.";

#[derive(Debug, Args)]
pub struct ConfineRunArgs {
    /// The writable workspace directory, mounted at /workspace; defaults to the current
    /// directory. Under linux its name must match [A-Za-z0-9_-]+
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// A host path bound read-only at the same path; repeat it, or pass a JSON list of paths. A
    /// relative path is resolved against the current directory lexically
    #[arg(long, value_name = "PATH", action = clap::ArgAction::Append, value_parser = parse_paths)]
    pub ro: Vec<Paths>,
    /// Ask for the host network; `--net` alone means true
    #[arg(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = parse_bool
    )]
    pub net: Option<bool>,
    /// The confinement adapter
    #[arg(long, value_enum, default_value_t = Backend::Linux)]
    pub backend: Backend,
    /// The image `oci` runs, named `<name>@sha256:<64 hex>`; only with `--backend oci`
    #[arg(long, value_name = "REFERENCE")]
    pub image: Option<String>,
    /// Admit the run as a real run would, print the program and arguments the adapter would run,
    /// one per line, and run nothing; descriptor numbers in them are local to the render.
    /// `--dry-run` alone means true
    #[arg(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = parse_bool
    )]
    pub dry_run: Option<bool>,
    /// Run unconfined under the declared-insecure `local` adapter, giving the reason
    #[arg(long, value_name = "REASON")]
    pub local: Option<String>,
    /// The program and its arguments, after `--`; required
    #[arg(last = true, required = true, num_args = 1..)]
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Backend {
    Linux,
    Oci,
}

/// One `--ro` value: a path, or a JSON list of paths.
#[derive(Debug, Clone)]
pub struct Paths(Vec<String>);

fn parse_paths(text: &str) -> Result<Paths, String> {
    if text.starts_with('[') {
        return serde_json::from_str(text)
            .map(Paths)
            .map_err(|_| "a value starting with [ is a JSON list of paths".to_owned());
    }
    Ok(Paths(vec![text.to_owned()]))
}

fn parse_bool(text: &str) -> Result<bool, String> {
    match text {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("true or false".to_owned()),
    }
}

/// The command `confine run` hands its adapter for `argv`: the argv and `sandbox`'s five baseline
/// variables, nothing else.
pub fn command(argv: Vec<String>) -> Command {
    Command {
        argv,
        env: BASELINE_ENV
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

/// The adapter the flags select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chosen {
    Local,
    Linux,
    Oci,
}

impl Chosen {
    fn of(args: &ConfineRunArgs) -> Self {
        match (&args.local, args.backend) {
            (Some(_), _) => Self::Local,
            (None, Backend::Linux) => Self::Linux,
            (None, Backend::Oci) => Self::Oci,
        }
    }

    const fn name(self) -> AdapterName {
        match self {
            Self::Local => AdapterName::Local,
            Self::Linux => AdapterName::Linux,
            Self::Oci => AdapterName::Oci,
        }
    }
}

/// Why the verb did not run its command to the end.
enum Failure {
    /// A named refusal of the confinement contract or of the host, before anything ran.
    Refused(ConfinementError),
    /// The host failed while running, or the invocation's own input is unusable.
    Failed(ConfinementError),
    Usage(String),
    /// `SIGINT` or `SIGTERM` reached the verb; the run, if one started, was killed.
    Interrupted(i32),
}

impl From<ConfinementError> for Failure {
    fn from(error: ConfinementError) -> Self {
        match &error {
            ConfinementError::Driver(driver) if driver.class == DriverErrorClass::Failed => {
                Self::Failed(error)
            }
            _ => Self::Refused(error),
        }
    }
}

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// `SIGINT` and `SIGTERM`, caught from before anything is made until the verb returns, so an
/// interrupted run removes what it made instead of leaving it behind.
struct Interrupts {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

impl Interrupts {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// The number of the next signal caught.
    async fn recv(&mut self) -> i32 {
        tokio::select! {
            _ = self.interrupt.recv() => SIGINT,
            _ = self.terminate.recv() => SIGTERM,
        }
    }
}

/// Runs `confine run` and returns its exit status.
pub fn run(output: Output, args: ConfineRunArgs) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return fail(
                output,
                &Failure::Usage(format!("starting the runtime: {error}")),
            );
        }
    };
    let outcome = runtime.block_on(async {
        let mut interrupts = Interrupts::new()
            .map_err(|error| Failure::Usage(format!("catching signals: {error}")))?;
        confine(output, args, &mut interrupts).await
    });
    match outcome {
        Ok(status) => status,
        Err(failure) => fail(output, &failure),
    }
}

async fn confine(
    output: Output,
    args: ConfineRunArgs,
    interrupts: &mut Interrupts,
) -> Result<ExitCode, Failure> {
    let chosen = Chosen::of(&args);
    // The image is checked before any adapter is chosen or asked anything: only `oci` runs one,
    // and only by digest. Anywhere else an image would be dropped, and the run would use the
    // host's own files.
    if args.image.is_some() && chosen != Chosen::Oci {
        return Err(Failure::Refused(ConfinementError::FactUnserved {
            adapter: chosen.name(),
            fact: Fact::ImageDigestPinned,
        }));
    }
    if chosen == Chosen::Oci && !args.image.as_deref().is_some_and(image_is_pinned) {
        return Err(Failure::Refused(ConfinementError::ImageUnpinned {
            adapter: AdapterName::Oci,
        }));
    }
    let workspace = match &args.dir {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()
            .map_err(|error| Failure::Usage(format!("the current directory: {error}")))?,
    };
    let workspace = std::fs::canonicalize(&workspace).map_err(|error| {
        Failure::Usage(format!("the workspace {}: {error}", workspace.display()))
    })?;
    if chosen == Chosen::Linux {
        check_workspace_name(&workspace)?;
    }
    let workspace_root = workspace
        .parent()
        .ok_or_else(|| Failure::Usage("the workspace cannot be the filesystem root".to_owned()))?
        .to_path_buf();
    let profile = profile(&args, workspace)?;
    // Private to this run and to its owner: a linux host's capsule and aperture state never lands
    // in the workspace's parent directory, and no other user can read or replace it.
    let state = tempfile::Builder::new()
        .prefix("b10x-substrate-confine-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|error| Failure::Usage(format!("a private state directory: {error}")))?;
    // Dropped after the adapter, so the driver is gone before its group is removed.
    let private = match chosen {
        Chosen::Linux => private_cgroup()?,
        Chosen::Local | Chosen::Oci => None,
    };
    let request = request(
        chosen,
        workspace_root,
        state.path(),
        private.as_ref().map(|group| group.path().to_path_buf()),
    );
    let adapter = confinement::open(request)?;
    let admitted = adapter.admit(&profile)?;
    let command = command(args.argv.clone());
    if output == Output::Human
        && let Some(reason) = admitted.insecure_reason()
    {
        write_line(
            &mut std::io::stderr(),
            &format!(
                "b10x-substrate: adapter={} declared_insecure={reason}",
                name(admitted.adapter())
            ),
        );
    }
    if args.dry_run == Some(true) {
        // The admission a run meets at start, beyond what a render needs: the adapter must run
        // processes at all. A dry run is refused wherever the run would be.
        if !adapter.facts().serves(&ContractFamily::ProcessRun) {
            return Err(Failure::Refused(ConfinementError::FamilyUnserved {
                adapter: adapter.facts().adapter.clone(),
                family: ContractFamily::ProcessRun,
            }));
        }
        let rendered = adapter.render(&admitted, &command)?;
        let program = rendered.program.to_string_lossy().into_owned();
        match output {
            Output::Json => write_line(
                &mut std::io::stdout(),
                &json!({"ok": true, "result": {
                    "adapter": name(admitted.adapter()),
                    "declared_insecure": admitted.insecure_reason(),
                    "program": program,
                    "args": rendered.args,
                    "note": DRY_RUN_NOTE,
                }})
                .to_string(),
            ),
            Output::Human => {
                let mut stdout = std::io::stdout().lock();
                for line in std::iter::once(&program).chain(&rendered.args) {
                    let _ = writeln!(stdout, "{line}");
                }
                let _ = stdout.flush();
                write_line(
                    &mut std::io::stderr(),
                    &format!("b10x-substrate: {DRY_RUN_NOTE}"),
                );
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let status = run_to_end(output, &adapter, &admitted, &command, interrupts).await;
    drop(adapter);
    drop(private);
    status
}

/// What a dry run says about its own output.
const DRY_RUN_NOTE: &str = "dry run: nothing ran; descriptor numbers in this argv are local to \
                            the render and name nothing once it returns";

/// The host's workspace identity rule (`HostDriver::workspace_path`): `linux` names a workspace
/// by its directory's name, which must be `[A-Za-z0-9_-]+`. Checked here so the user meets the
/// rule, not the host's path-escape refusal it would otherwise surface as.
fn check_workspace_name(workspace: &Path) -> Result<(), Failure> {
    let name = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(Failure::Usage(format!(
            "--dir {}: under linux a workspace directory's name must match [A-Za-z0-9_-]+ (the \
             host's workspace identity rule); {name:?} does not",
            workspace.display()
        )));
    }
    Ok(())
}

/// The private group of [`CGROUP_ROOT_ENV`], or none when it is unset.
fn private_cgroup() -> Result<Option<PrivateCgroup>, Failure> {
    let Some(root) = std::env::var_os(CGROUP_ROOT_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let root = PathBuf::from(root);
    PrivateCgroup::create(&root).map(Some).map_err(|reason| {
        Failure::Refused(ConfinementError::Driver(DriverError::unserved(
            "exec.sandbox-unavailable",
            format!(
                "{CGROUP_ROOT_ENV}={}: no private cgroup can be made beneath it: {reason}",
                root.display()
            ),
            CGROUP_ROOT_ENV,
        )))
    })
}

/// `path` made absolute against the current directory, with `.` and `..` resolved lexically as
/// `sandbox` does (`src/main.rs` at `1b0377d`), so `../lib` is usable from a shell. Symlinks are
/// left alone: the host's canonical check still refuses a root reached through one.
fn lexically_absolute(path: &Path) -> std::io::Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

fn profile(args: &ConfineRunArgs, workspace: PathBuf) -> Result<ConfinementProfile, Failure> {
    let mut read_only_roots = Vec::new();
    for path in args.ro.iter().flat_map(|paths| &paths.0) {
        let absolute = lexically_absolute(Path::new(path))
            .map_err(|error| Failure::Usage(format!("--ro {path}: {error}")))?;
        let text = absolute
            .to_str()
            .ok_or_else(|| Failure::Usage(format!("--ro {path}: not UTF-8")))?
            .to_owned();
        read_only_roots.push(ReadOnlyRoot {
            host_path: text.clone(),
            mount: text,
        });
    }
    let net = args.net == Some(true);
    Ok(ConfinementProfile {
        workspace: Some(workspace),
        read_only_roots,
        // The host network is a fact the adapter must prove; without it the run has none.
        network: if net { None } else { Some(Network::Isolated) },
        facts: if net {
            vec![Fact::HostNetwork]
        } else {
            Vec::new()
        },
        local_reason: args.local.clone(),
        image: args.image.clone(),
        ..ConfinementProfile::default()
    })
}

fn request(
    chosen: Chosen,
    workspace_root: PathBuf,
    state: &Path,
    cgroup_root: Option<PathBuf>,
) -> AdapterRequest {
    match chosen {
        Chosen::Local => AdapterRequest::Local(LocalRequest::default()),
        Chosen::Linux => {
            let mut config = HostConfig::minimum(workspace_root);
            config.capsule_root = state.join("capsules");
            config.aperture_root = state.join("apertures");
            AdapterRequest::Linux(Box::new(
                LinuxRequest::new(config).with_cgroup_root(cgroup_root),
            ))
        }
        Chosen::Oci => AdapterRequest::Oci(OciRequest {
            runtime: PathBuf::from("docker"),
            workspace_root,
            writable_roots: Vec::new(),
        }),
    }
}

async fn run_to_end(
    output: Output,
    adapter: &Arc<dyn ConfinementAdapter>,
    admitted: &AdmittedProfile,
    command: &Command,
    interrupts: &mut Interrupts,
) -> Result<ExitCode, Failure> {
    let run = adapter.start(admitted, command).await?;
    let ended = tokio::select! {
        ended = adapter.wait(&run) => ended?,
        signal = interrupts.recv() => {
            let _ = adapter.signal(&run, Signal::Kill).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), adapter.wait(&run)).await;
            return Err(Failure::Interrupted(signal));
        }
    };
    let ran = adapter.output(&run).await?;
    let status = exit_status(&ended);
    match output {
        Output::Json => write_line(
            &mut std::io::stdout(),
            &json!({"ok": true, "result": {
                "adapter": name(&ended.adapter),
                "declared_insecure": ended.declared_insecure,
                "exit_code": ended.exit.and_then(|exit| exit.code),
                "signal": ended.exit.and_then(|exit| exit.signal),
                "stdout": String::from_utf8_lossy(&ran.stdout),
                "stderr": String::from_utf8_lossy(&ran.stderr),
                "stdout_truncated": ran.stdout_truncated,
                "stderr_truncated": ran.stderr_truncated,
            }})
            .to_string(),
        ),
        Output::Human => {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(&ran.stdout);
            let _ = stdout.flush();
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(&ran.stderr);
            if ended.exit.is_none() {
                let _ = writeln!(
                    stderr,
                    "b10x-substrate: the run ended without an exit status"
                );
            }
            for (stream, truncated) in [
                ("stdout", ran.stdout_truncated),
                ("stderr", ran.stderr_truncated),
            ] {
                if truncated {
                    let _ = writeln!(stderr, "b10x-substrate: {stream} was truncated");
                }
            }
            let _ = stderr.flush();
        }
    }
    Ok(ExitCode::from(status))
}

/// The run's exit code, 128 plus the signal number for a signal, and a failure without either.
fn exit_status(observation: &RunObservation) -> u8 {
    match observation.exit {
        Some(exit) => match (exit.code, exit.signal) {
            (Some(code), _) => u8::try_from(code & 0xff).unwrap_or(EXIT_FAILURE),
            (None, Some(signal)) => u8::try_from(128 + (signal & 0x7f)).unwrap_or(EXIT_FAILURE),
            (None, None) => EXIT_FAILURE,
        },
        None => EXIT_FAILURE,
    }
}

/// A serialized name: `HostNetwork` reads `host_network`.
fn name(value: &impl serde::Serialize) -> String {
    match to_json(value) {
        Value::String(name) => name,
        other => other.to_string(),
    }
}

/// What a refusal names after its code: the fact, family or field it is about, otherwise the
/// adapter's own words.
fn detail(error: &ConfinementError) -> (String, Option<String>) {
    match error {
        ConfinementError::FactUnserved { fact, .. } => (name(fact), None),
        ConfinementError::FamilyUnserved { family, .. } => (name(family), None),
        ConfinementError::ReservedFieldUnserved { field } => (name(field), None),
        ConfinementError::AdapterUnserved { adapter, detail } => {
            (format!("{}: {detail}", name(adapter)), None)
        }
        ConfinementError::LocalReasonMissing { .. } => (
            "local runs declared insecure and needs a reason".to_owned(),
            None,
        ),
        ConfinementError::ImageUnpinned { .. } => {
            ("an image is named <name>@sha256:<64 hex>".to_owned(), None)
        }
        ConfinementError::LocalRefused { .. } => {
            ("the host's policy forbids local".to_owned(), None)
        }
        ConfinementError::Driver(driver) => (driver.message.clone(), driver.address.clone()),
    }
}

fn fail(output: Output, failure: &Failure) -> ExitCode {
    let (code, data, status, human) = match failure {
        Failure::Refused(error) | Failure::Failed(error) => {
            let (message, address) = detail(error);
            let status = if matches!(failure, Failure::Refused(_)) {
                EXIT_USAGE
            } else {
                EXIT_FAILURE
            };
            (
                if status == EXIT_USAGE {
                    "refused"
                } else {
                    "failed"
                },
                json!({
                    "code": error.code(),
                    "message": message,
                    "status": error.status(),
                    "address": address,
                }),
                status,
                format!(
                    "{}: {message}{}",
                    error.code(),
                    address
                        .map(|address| format!(" ({address})"))
                        .unwrap_or_default()
                ),
            )
        }
        Failure::Usage(message) => (
            "cli_input",
            json!({"message": message}),
            EXIT_USAGE,
            message.clone(),
        ),
        Failure::Interrupted(signal) => {
            let signal_name = if *signal == SIGINT {
                "SIGINT"
            } else {
                "SIGTERM"
            };
            let message = format!("{signal_name} ended the verb; the run was killed");
            (
                "failed",
                json!({
                    "code": "confine.interrupted",
                    "message": message,
                    "status": 500,
                    "address": null,
                }),
                u8::try_from(128 + signal).unwrap_or(EXIT_FAILURE),
                format!("confine.interrupted: {message}"),
            )
        }
    };
    let text = match output {
        Output::Json => json!({"ok": false, "error": {"code": code, "data": data}}).to_string(),
        Output::Human => format!("b10x-substrate: {human}"),
    };
    write_line(&mut std::io::stderr(), &text);
    ExitCode::from(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_resolves_dot_and_dotdot_lexically() {
        let cwd = std::env::current_dir().expect("cwd");
        let parent = cwd.parent().expect("parent").to_path_buf();
        assert_eq!(
            lexically_absolute(Path::new("../tool/./x")).expect("absolute"),
            parent.join("tool").join("x")
        );
        assert_eq!(
            lexically_absolute(Path::new("/a/b/../c/.")).expect("absolute"),
            PathBuf::from("/a/c")
        );
    }

    #[test]
    fn the_workspace_name_rule_is_the_hosts() {
        for good in ["ws", "a-b_C9"] {
            assert!(check_workspace_name(&Path::new("/x").join(good)).is_ok());
        }
        for bad in ["my.project", "a b", "é"] {
            assert!(check_workspace_name(&Path::new("/x").join(bad)).is_err());
        }
    }
}
