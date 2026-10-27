//! `b10x-substrate`: a command line that drives a Substrate daemon over its Unix socket.
//!
//! The command line is the ESS CLI binding `spec/client/cli.yaml`, held to its resolved form
//! `spec/generated/cli/binding.json` by the `cli_binding_validates` test. Every command is one
//! call through `b10x-substrate-sdk`; the CLI adds no route, wire field or daemon behaviour.
//!
//! Output follows the binding's envelope. With `--output json` a success is
//! `{"ok":true,"result":…}` on stdout and a failure `{"ok":false,"error":{"code":…,"data":…}}` on
//! stderr; a daemon refusal is the CLI error `refused` with the daemon's named code in
//! `error.data.code`. Human output is a line or a table, and a refusal names its code.
//!
//! `confine run` is the one verb that needs no daemon: it opens a confinement adapter in this
//! process ([`confine`]).

mod confine;

pub use confine::{Backend, ConfineRunArgs};

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use b10x_substrate_sdk::{
    Client, ExecObservation, ExecState, ExecutionPolicy, MAX_EVENT_PAGE_ITEMS, Machine,
    MetricsResourceKind, Refusal, RefusalClass, SdkError, Signal, StorageLimit,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};

/// The environment variable read when `--socket` is absent: the daemon's own.
pub const SOCKET_ENV: &str = "SUBSTRATE_SOCKET";

/// Exit status of a usage error: an absent socket, an invalid argument.
const EXIT_USAGE: u8 = 2;
/// Exit status of a daemon refusal or an unavailable daemon.
const EXIT_FAILURE: u8 = 1;

/// Drive a Substrate daemon over its Unix socket
#[derive(Debug, Parser)]
#[command(name = "b10x-substrate", version)]
pub struct Cli {
    /// Daemon socket path; falls back to `SUBSTRATE_SOCKET`
    #[arg(long, global = true, value_name = "PATH")]
    pub socket: Option<PathBuf>,
    /// Output form
    #[arg(long, global = true, value_enum, default_value_t = Output::Human)]
    pub output: Output,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Output {
    Human,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Confine(ConfineCommand),
    /// Read one page of the daemon's event log
    Events(EventsArgs),
    #[command(subcommand)]
    Exec(ExecCommand),
    /// Show the daemon's capability snapshot and facts
    Machine,
    /// Show the resource usage the daemon observes for an exec or a workspace
    Metrics(MetricsArgs),
    #[command(subcommand)]
    Operation(OperationCommand),
    #[command(subcommand)]
    Session(SessionCommand),
    #[command(subcommand)]
    Workspace(WorkspaceCommand),
}

#[derive(Debug, Subcommand)]
pub enum ConfineCommand {
    /// Run a command under a confinement adapter in this process, without a daemon
    Run(ConfineRunArgs),
}

#[derive(Debug, Args)]
pub struct EventsArgs {
    /// Resume cursor from a previous page's `next_cursor`
    #[arg(long)]
    pub cursor: Option<String>,
    /// Largest number of events to read (1..=1000)
    #[arg(
        long,
        default_value_t = 100,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_EVENT_PAGE_ITEMS))
    )]
    pub limit: u32,
}

#[derive(Debug, Subcommand)]
pub enum ExecCommand {
    /// Run a command in a workspace and wait for it to end
    Run(ExecRunArgs),
    /// Show an exec as the daemon observes it
    Show {
        /// Exec id
        exec: String,
    },
}

#[derive(Debug, Args)]
pub struct ExecRunArgs {
    /// Workspace id
    #[arg(long)]
    pub workspace: String,
    /// Wall-clock limit in milliseconds
    #[arg(long, default_value_t = 30_000)]
    pub timeout_ms: u64,
    /// Cumulative CPU limit in milliseconds
    #[arg(long, default_value_t = 30_000)]
    pub cpu_millis: u64,
    /// Memory and swap ceiling in bytes
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    pub memory_bytes: u64,
    /// Largest process count
    #[arg(long, default_value_t = 64)]
    pub processes: u32,
    /// Largest recorded output per stream in bytes
    #[arg(long, default_value_t = 1024 * 1024)]
    pub output_bytes: u64,
    /// Client-chosen idempotency key
    #[arg(long)]
    pub operation_id: Option<String>,
    /// The program and its arguments, after `--`
    #[arg(last = true, required = true, num_args = 1..)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct MetricsArgs {
    /// Resource kind
    #[arg(long = "kind", value_enum)]
    pub resource_kind: ResourceKind,
    /// Resource id
    #[arg(long = "id")]
    pub resource_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ResourceKind {
    Exec,
    Workspace,
}

#[derive(Debug, Subcommand)]
pub enum OperationCommand {
    /// Show an accepted operation's ledger record
    Show {
        /// Operation id
        operation: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Show a session as the daemon observes it
    Show {
        /// Session id
        session: String,
    },
    /// Signal a session's process
    Signal(SessionSignalArgs),
}

#[derive(Debug, Args)]
pub struct SessionSignalArgs {
    /// Session id
    pub session: String,
    /// Signal to deliver
    #[arg(long, value_enum)]
    pub signal: SignalName,
    /// Grace before the whole tree is killed, in milliseconds
    #[arg(long, default_value_t = 5_000)]
    pub grace_ms: u64,
    /// Client-chosen idempotency key
    #[arg(long)]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SignalName {
    Int,
    Term,
    Kill,
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Create an empty workspace
    Create(WorkspaceCreateArgs),
    /// Destroy a workspace
    Destroy {
        /// Workspace id
        workspace: String,
        /// Client-chosen idempotency key
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Renew a workspace lease
    Renew {
        /// Workspace id
        workspace: String,
        /// New lease length in milliseconds
        #[arg(long)]
        ttl_ms: u64,
        /// Client-chosen idempotency key
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Show a workspace as the daemon observes it
    Show {
        /// Workspace id
        workspace: String,
    },
}

#[derive(Debug, Args)]
pub struct WorkspaceCreateArgs {
    /// Labels as a JSON object of strings, e.g. '{"team":"a"}'
    #[arg(long, value_parser = parse_labels)]
    pub labels: Option<BTreeMap<String, String>>,
    /// Hard storage quota as JSON, e.g. `{"max_bytes":1048576,"max_inodes":1024}`
    #[arg(long, value_parser = parse_storage)]
    pub storage: Option<StorageLimit>,
    /// Lease length in milliseconds
    #[arg(long)]
    pub lease_ttl_ms: Option<u64>,
    /// Client-chosen idempotency key
    #[arg(long)]
    pub operation_id: Option<String>,
}

fn parse_labels(text: &str) -> Result<BTreeMap<String, String>, String> {
    serde_json::from_str(text).map_err(|_| "labels are a JSON object of strings".to_owned())
}

fn parse_storage(text: &str) -> Result<StorageLimit, String> {
    serde_json::from_str(text)
        .map_err(|_| "storage is a JSON object with max_bytes and max_inodes".to_owned())
}

/// Every fact named here appears in the human `machine` table, as `absent` when the daemon does
/// not advertise it. Facts the daemon does advertise appear whatever this list holds.
const HEADLINE_FACTS: [&str; 9] = [
    "exec.namespaces",
    "exec.cgroup-limits",
    "exec.cgroup-kill",
    "exec.no-egress",
    "exec.argv-only",
    "workspace.guarded-io",
    "events.pull",
    "events.stream",
    "sessions.pty",
];

/// What a command answers: a typed result for the envelope and its human rendering, or a raw
/// exec run whose bytes and status are the command's.
enum Answer {
    Result { result: Value, human: String },
    Run(Box<RunAnswer>),
}

struct RunAnswer {
    exec: ExecObservation,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

/// Why a command did not answer.
enum Failure {
    /// The daemon refused, with its named code.
    Refused(Refusal),
    /// The daemon could not be reached or answered outside the contract.
    Unavailable(&'static str, String),
    /// The invocation itself is unusable.
    Usage(String),
}

impl From<SdkError> for Failure {
    fn from(error: SdkError) -> Self {
        match error {
            SdkError::Refusal(refusal) => Self::Refused(refusal),
            SdkError::Builder { field } => Self::Usage(format!("{field} is required")),
            error @ SdkError::Transport(_) => Self::Unavailable("transport", error.to_string()),
            error @ SdkError::ContractMismatch { .. } => {
                Self::Unavailable("contract_mismatch", error.to_string())
            }
            error @ SdkError::UnknownOperation { .. } => {
                Self::Unavailable("unknown_operation", error.to_string())
            }
            error @ SdkError::EventGap { .. } => Self::Unavailable("event_gap", error.to_string()),
            error => Self::Unavailable("protocol", error.to_string()),
        }
    }
}

/// Runs the CLI on `args` and returns its exit status.
pub fn main_with(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args: Vec<OsString> = args.into_iter().collect();
    let wants_json = asks_for_json(&args);
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => return usage_error(&error, wants_json),
    };
    let output = cli.output;
    let command = match cli.command {
        Command::Confine(ConfineCommand::Run(args)) => return confine::run(output, args),
        command => command,
    };
    let socket = cli.socket.clone().or_else(|| {
        std::env::var_os(SOCKET_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    });
    let Some(socket) = socket else {
        return fail(
            output,
            &Failure::Usage(format!(
                "no daemon socket: pass --socket <path> or set {SOCKET_ENV}"
            )),
        );
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return fail(
                output,
                &Failure::Unavailable("transport", format!("starting the runtime: {error}")),
            );
        }
    };
    match runtime.block_on(run(socket, command)) {
        Ok(Answer::Result { result, human }) => {
            let text = match output {
                Output::Json => json!({"ok": true, "result": result}).to_string(),
                Output::Human => human,
            };
            write_line(&mut std::io::stdout(), &text);
            ExitCode::SUCCESS
        }
        Ok(Answer::Run(run)) => report_run(output, &run),
        Err(failure) => fail(output, &failure),
    }
}

/// Whether the words ask for `--output json`, as `--output json` or `--output=json`, before any
/// `--`. A parse that failed cannot say, so this reads the words themselves.
fn asks_for_json(args: &[OsString]) -> bool {
    let mut words = args.iter().skip(1).map(|word| word.to_string_lossy());
    let mut json = false;
    while let Some(word) = words.next() {
        match word.as_ref() {
            "--" => break,
            "--output" => json = words.next().is_some_and(|value| value == "json"),
            "--output=json" => json = true,
            other if other.starts_with("--output=") => json = false,
            _ => {}
        }
    }
    json
}

/// A clap parse failure. Help and version keep clap's output; under `--output json` every other
/// failure is one `cli_input` envelope on stderr with exit 2, and otherwise clap's own text.
fn usage_error(error: &clap::Error, wants_json: bool) -> ExitCode {
    use clap::error::ErrorKind;
    if wants_json
        && !matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        )
    {
        let message = error.to_string();
        return fail(Output::Json, &Failure::Usage(message.trim_end().to_owned()));
    }
    error.exit()
}

async fn run(socket: PathBuf, command: Command) -> Result<Answer, Failure> {
    let client = Client::builder().unix_socket(socket).connect().await?;
    match command {
        Command::Machine => machine(&client).await,
        Command::Workspace(command) => workspace(&client, command).await,
        Command::Exec(ExecCommand::Run(args)) => exec_run(&client, args).await,
        Command::Exec(ExecCommand::Show { exec }) => {
            let exec = client.get_exec(&exec).await?;
            let observation = exec.observation();
            Ok(Answer::Result {
                result: json!({"exec": observation.id, "observation": to_json(observation)}),
                human: exec_line(observation),
            })
        }
        Command::Session(SessionCommand::Show { session }) => {
            let session = client.get_pipe_session(&session).await?;
            Ok(session_answer(session.observation()))
        }
        Command::Session(SessionCommand::Signal(args)) => {
            let mut session = client.get_pipe_session(&args.session).await?;
            let signal = match args.signal {
                SignalName::Int => Signal::Interrupt,
                SignalName::Term => Signal::Terminate,
                SignalName::Kill => Signal::Kill,
            };
            let observation = session
                .signal_with_operation_id(
                    signal,
                    Duration::from_millis(args.grace_ms),
                    args.operation_id,
                )
                .await?;
            Ok(session_answer(observation))
        }
        Command::Operation(OperationCommand::Show { operation }) => {
            let record = client.operation(&operation).await?;
            let mut human = format!(
                "operation {} {} {} resource {}",
                record.id,
                record.kind,
                name_of(&record.state),
                record.resource.as_deref().unwrap_or("-")
            );
            if let Some(refusal) = &record.refusal {
                let _ = write!(human, " refused {}", refusal.code);
            }
            Ok(Answer::Result {
                result: json!({"operation": record.id, "observation": to_json(&record)}),
                human,
            })
        }
        Command::Metrics(args) => {
            let (kind, name) = match args.resource_kind {
                ResourceKind::Exec => (MetricsResourceKind::Exec, "exec"),
                ResourceKind::Workspace => (MetricsResourceKind::Workspace, "workspace"),
            };
            let observed = client.metrics(kind, &args.resource_id).await?;
            let observation = to_json(&observed);
            Ok(Answer::Result {
                human: format!("metrics {name} {} {observation}", args.resource_id),
                result: json!({
                    "resource_kind": name,
                    "resource_id": args.resource_id,
                    "observation": observation,
                }),
            })
        }
        Command::Events(args) => events(&client, args).await,
        // Dispatched by `main_with` before a socket is needed.
        Command::Confine(_) => Err(Failure::Usage(
            "confine run needs no daemon socket".to_owned(),
        )),
    }
}

async fn machine(client: &Client) -> Result<Answer, Failure> {
    let machine: Machine = client.refresh_machine().await?;
    // The verified fact set, verbatim: an unadvertised fact stays absent, never false.
    let facts = to_json(&machine.facts);
    let observation = json!({
        "snapshot": machine.capability_snapshot,
        "driver_version": machine.driver_version,
        "configuration_generation": machine.configuration_generation,
        "probed_at": machine.probed_at,
        "valid_until": machine.valid_until,
        "facts": facts,
    });
    let mut rows: BTreeMap<String, String> = BTreeMap::new();
    if let Value::Object(map) = &facts {
        for (name, value) in map {
            rows.insert(name.clone(), value.to_string());
        }
    }
    for name in HEADLINE_FACTS {
        rows.entry(name.to_owned())
            .or_insert_with(|| "absent".to_owned());
    }
    let width = rows.keys().map(String::len).max().unwrap_or(4).max(4);
    let mut human = format!(
        "snapshot {}\ndriver {}\n{:width$}  VALUE",
        machine.capability_snapshot, machine.driver_version, "FACT"
    );
    for (name, value) in rows {
        let _ = write!(human, "\n{name:width$}  {value}");
    }
    Ok(Answer::Result {
        result: json!({"snapshot": machine.capability_snapshot, "observation": observation}),
        human,
    })
}

async fn workspace(client: &Client, command: WorkspaceCommand) -> Result<Answer, Failure> {
    match command {
        WorkspaceCommand::Create(args) => {
            let mut builder = client.workspace().empty();
            for (key, value) in args.labels.unwrap_or_default() {
                builder = builder.label(key, value);
            }
            if let Some(storage) = args.storage {
                builder = builder.storage(storage);
            }
            if let Some(ttl) = args.lease_ttl_ms {
                builder = builder.lease(Duration::from_millis(ttl));
            }
            if let Some(operation_id) = args.operation_id {
                builder = builder.operation_id(operation_id);
            }
            let workspace = builder.create().await?;
            Ok(workspace_answer(workspace.observation()))
        }
        WorkspaceCommand::Show { workspace } => {
            let workspace = client.get_workspace(&workspace).await?;
            Ok(workspace_answer(workspace.observation()))
        }
        WorkspaceCommand::Renew {
            workspace,
            ttl_ms,
            operation_id,
        } => {
            let mut workspace = client.get_workspace(&workspace).await?;
            let observation = workspace
                .renew_lease_with_operation_id(Duration::from_millis(ttl_ms), operation_id)
                .await?;
            Ok(workspace_answer(observation))
        }
        WorkspaceCommand::Destroy {
            workspace,
            operation_id,
        } => {
            let observed = client.get_workspace(&workspace).await?;
            let id = observed.id().to_owned();
            let absent = observed.destroy_with_operation_id(operation_id).await?;
            Ok(Answer::Result {
                human: format!(
                    "workspace {id} {}",
                    if absent { "absent" } else { "destroying" }
                ),
                result: json!({"workspace": id, "observation": {"id": id, "absent": absent}}),
            })
        }
    }
}

async fn exec_run(client: &Client, args: ExecRunArgs) -> Result<Answer, Failure> {
    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_millis(args.timeout_ms))
        .cpu_time(Duration::from_millis(args.cpu_millis))
        .memory_bytes(args.memory_bytes)
        .processes(args.processes)
        .output_bytes(args.output_bytes)
        .build()
        .map_err(|error| match error {
            SdkError::Refusal(refusal) => Failure::Refused(refusal),
            error => Failure::Usage(error.to_string()),
        })?;
    let workspace = client.get_workspace(&args.workspace).await?;
    let mut words = args.argv.into_iter();
    let program = words.next().unwrap_or_default();
    let mut command = workspace.command(program).args(words).policy(policy);
    if let Some(operation_id) = args.operation_id {
        command = command.operation_id(operation_id);
    }
    let output = command.run().await?;
    Ok(Answer::Run(Box::new(RunAnswer {
        exec: output.exec,
        stdout: output.stdout,
        stderr: output.stderr,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
    })))
}

async fn events(client: &Client, args: EventsArgs) -> Result<Answer, Failure> {
    let page = client.events(args.cursor.as_deref(), args.limit).await?;
    let mut human = String::from("SEQ  TRANSITION  KIND  RESOURCE");
    for event in &page.events {
        let _ = write!(
            human,
            "\n{}  {}  {}  {}",
            event.sequence, event.transition, event.resource_kind, event.resource
        );
    }
    let _ = write!(human, "\nnext_cursor {}", page.next_cursor);
    let items: Vec<Value> = page.events.iter().map(to_json).collect();
    Ok(Answer::Result {
        result: json!({
            "source_scope": page.source_scope,
            "generation": page.generation,
            "items": items,
            "next_cursor": page.next_cursor,
            "through_seq": page.through_sequence,
            "first_retained_seq": page.first_retained_sequence,
        }),
        human,
    })
}

fn workspace_answer(observation: &b10x_substrate_sdk::WorkspaceObservation) -> Answer {
    let lease = observation
        .lease
        .as_ref()
        .map_or_else(|| "-".to_owned(), |lease| lease.renew_by.to_rfc3339());
    Answer::Result {
        human: format!(
            "workspace {} {} renew_by {lease}",
            observation.id,
            name_of(&observation.state)
        ),
        result: json!({"workspace": observation.id, "observation": to_json(observation)}),
    }
}

fn session_answer(observation: &b10x_substrate_sdk::PipeSessionObservation) -> Answer {
    Answer::Result {
        human: format!(
            "session {} {} exec {}",
            observation.id,
            name_of(&observation.state),
            observation.exec_id
        ),
        result: json!({"session": observation.id, "observation": to_json(observation)}),
    }
}

fn exec_line(observation: &ExecObservation) -> String {
    let exit = match &observation.exit {
        Some(exit) => match (exit.code, exit.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("signal {}", name_of(&signal)),
            (None, None) => "exit -".to_owned(),
        },
        None => "exit -".to_owned(),
    };
    let mut line = format!(
        "exec {} {} {exit}",
        observation.id,
        name_of(&observation.state)
    );
    if let Some(refusal) = &observation.refusal {
        let _ = write!(line, " refused {}", refusal.code);
    }
    line
}

/// The exit status a finished exec maps to: its code, 128 plus the signal number for a signal,
/// and a failure when it ended without either.
fn exec_status(observation: &ExecObservation) -> u8 {
    match &observation.exit {
        Some(exit) => match (exit.code, exit.signal) {
            (Some(code), _) => code,
            (None, Some(signal)) => 128 + signal_number(signal),
            (None, None) => EXIT_FAILURE,
        },
        None => EXIT_FAILURE,
    }
}

const fn signal_number(signal: Signal) -> u8 {
    match signal {
        Signal::Interrupt => 2,
        Signal::Kill => 9,
        Signal::Terminate => 15,
    }
}

fn report_run(output: Output, run: &RunAnswer) -> ExitCode {
    let status = exec_status(&run.exec);
    match output {
        Output::Json => {
            let result = json!({
                "exec": run.exec.id,
                "observation": to_json(&run.exec),
                "stdout": String::from_utf8_lossy(&run.stdout),
                "stderr": String::from_utf8_lossy(&run.stderr),
                "stdout_truncated": run.stdout_truncated,
                "stderr_truncated": run.stderr_truncated,
            });
            write_line(
                &mut std::io::stdout(),
                &json!({"ok": true, "result": result}).to_string(),
            );
        }
        Output::Human => {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(&run.stdout);
            let _ = stdout.flush();
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(&run.stderr);
            if run.exec.exit.is_none()
                || run.exec.refusal.is_some()
                || !matches!(run.exec.state, ExecState::Exited)
            {
                let _ = writeln!(stderr, "b10x-substrate: {}", exec_line(&run.exec));
            }
            for (name, truncated) in [
                ("stdout", run.stdout_truncated),
                ("stderr", run.stderr_truncated),
            ] {
                if truncated {
                    let _ = writeln!(stderr, "b10x-substrate: {name} was truncated");
                }
            }
            let _ = stderr.flush();
        }
    }
    ExitCode::from(status)
}

fn fail(output: Output, failure: &Failure) -> ExitCode {
    let (code, data, status, human) = match failure {
        Failure::Refused(refusal) => (
            "refused",
            json!({
                "class": refusal_class(refusal.class),
                "code": refusal.code,
                "message": refusal.message,
                "retriable": refusal.retriable,
                "address": refusal.address,
                "operation": refusal.operation_id,
            }),
            EXIT_FAILURE,
            format!(
                "refused {}: {}{}",
                refusal.code,
                refusal.message,
                refusal
                    .address
                    .as_deref()
                    .map(|address| format!(" ({address})"))
                    .unwrap_or_default()
            ),
        ),
        Failure::Unavailable(reason, message) => (
            "unavailable",
            json!({"reason": reason}),
            EXIT_FAILURE,
            format!("unavailable ({reason}): {message}"),
        ),
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

const fn refusal_class(class: RefusalClass) -> &'static str {
    match class {
        RefusalClass::Refused => "refused",
        RefusalClass::Conflict => "conflict",
        RefusalClass::Unserved => "unserved",
        RefusalClass::Exhausted => "exhausted",
        RefusalClass::Failed => "failed",
    }
}

fn to_json(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A serialized unit variant, lowercased: `Ready` reads `ready`.
fn name_of(value: &impl Serialize) -> String {
    match to_json(value) {
        Value::String(name) => name.to_lowercase(),
        other => other.to_string(),
    }
}

fn write_line(stream: &mut impl std::io::Write, text: &str) {
    let _ = writeln!(stream, "{text}");
    let _ = stream.flush();
}
