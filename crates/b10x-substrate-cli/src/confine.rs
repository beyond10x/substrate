//! `b10x-substrate confine run`: the `sandbox` repository's command line over Substrate's
//! confinement adapters (ADR 0109), without a daemon.
//!
//! The verb builds one [`ConfinementProfile`] from its flags, opens the adapter through
//! [`confinement::open`] and lets that adapter admit, render or run it. It adds no confinement of
//! its own: every refusal is the adapter's named refusal, and every one exits 2 before anything
//! runs. `--local <reason>` selects the declared-insecure `local` adapter, and the verb says so
//! on stderr.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, ValueEnum};
use serde_json::{Value, json};
use substrate_host::DriverErrorClass;
use substrate_host::confinement::{
    self, AdapterRequest, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, Fact, HostConfig, Network, ReadOnlyRoot, RunObservation, image_is_pinned,
    linux::LinuxRequest, local::LocalRequest, oci::OciRequest,
};

use crate::{EXIT_FAILURE, EXIT_USAGE, Output, to_json, write_line};

/// The variable naming the delegated cgroup v2 root, the daemon's own.
pub const CGROUP_ROOT_ENV: &str = "SUBSTRATE_CGROUP_ROOT";

/// The `PATH` a confined command starts with: nothing else is inherited from this process.
const COMMAND_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Debug, Args)]
pub struct ConfineRunArgs {
    /// The writable workspace directory, mounted at /workspace; defaults to the current directory
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// A host path bound read-only at the same path; repeat it, or pass a JSON list of paths
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
    /// The image `oci` runs, named `<name>@sha256:<64 hex>`
    #[arg(long, value_name = "REFERENCE")]
    pub image: Option<String>,
    /// Print the program and arguments the adapter would run, one per line, and run nothing;
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
    /// The program and its arguments, after `--`
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

/// Why the verb did not run its command to the end.
enum Failure {
    /// A named refusal of the confinement contract or of the host, before anything ran.
    Refused(ConfinementError),
    /// The host failed while running, or the invocation's own input is unusable.
    Failed(ConfinementError),
    Usage(String),
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
    match runtime.block_on(confine(output, args)) {
        Ok(status) => status,
        Err(failure) => fail(output, &failure),
    }
}

async fn confine(output: Output, args: ConfineRunArgs) -> Result<ExitCode, Failure> {
    let workspace = match &args.dir {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()
            .map_err(|error| Failure::Usage(format!("the current directory: {error}")))?,
    };
    let workspace = std::fs::canonicalize(&workspace).map_err(|error| {
        Failure::Usage(format!("the workspace {}: {error}", workspace.display()))
    })?;
    let workspace_root = workspace
        .parent()
        .ok_or_else(|| Failure::Usage("the workspace cannot be the filesystem root".to_owned()))?
        .to_path_buf();
    let profile = profile(&args, workspace)?;
    // A runtime is never asked about an image it would refuse to run.
    if args.local.is_none()
        && args.backend == Backend::Oci
        && !profile.image.as_deref().is_some_and(image_is_pinned)
    {
        return Err(Failure::Refused(ConfinementError::ImageUnpinned {
            adapter: confinement::AdapterName::Oci,
        }));
    }
    // Private to this run: a linux host's capsule and aperture state never lands in the
    // workspace's parent directory.
    let state = tempfile::Builder::new()
        .prefix("b10x-substrate-confine-")
        .tempdir()
        .map_err(|error| Failure::Usage(format!("a private state directory: {error}")))?;
    let request = request(&args, workspace_root, state.path());
    let adapter = confinement::open(request)?;
    let admitted = adapter.admit(&profile)?;
    let command = Command {
        argv: args.argv.clone(),
        env: [("PATH".to_owned(), COMMAND_PATH.to_owned())].into(),
    };
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
                }})
                .to_string(),
            ),
            Output::Human => {
                let mut stdout = std::io::stdout().lock();
                for line in std::iter::once(&program).chain(&rendered.args) {
                    let _ = writeln!(stdout, "{line}");
                }
                let _ = stdout.flush();
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    run_to_end(output, &adapter, &admitted, &command).await
}

fn profile(args: &ConfineRunArgs, workspace: PathBuf) -> Result<ConfinementProfile, Failure> {
    let mut read_only_roots = Vec::new();
    for path in args.ro.iter().flat_map(|paths| &paths.0) {
        let absolute = std::path::absolute(path)
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

fn request(args: &ConfineRunArgs, workspace_root: PathBuf, state: &Path) -> AdapterRequest {
    if args.local.is_some() {
        return AdapterRequest::Local(LocalRequest::default());
    }
    match args.backend {
        Backend::Linux => {
            let mut config = HostConfig::minimum(workspace_root);
            config.capsule_root = state.join("capsules");
            config.aperture_root = state.join("apertures");
            let cgroup_root = std::env::var_os(CGROUP_ROOT_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            AdapterRequest::Linux(Box::new(
                LinuxRequest::new(config).with_cgroup_root(cgroup_root),
            ))
        }
        Backend::Oci => AdapterRequest::Oci(OciRequest {
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
) -> Result<ExitCode, Failure> {
    let run = adapter.start(admitted, command).await?;
    let ended = adapter.wait(&run).await?;
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
    };
    let text = match output {
        Output::Json => json!({"ok": false, "error": {"code": code, "data": data}}).to_string(),
        Output::Human => format!("b10x-substrate: {human}"),
    };
    write_line(&mut std::io::stderr(), &text);
    ExitCode::from(status)
}
