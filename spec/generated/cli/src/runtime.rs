//! Generated process adapter. Application behavior enters only through explicit seams.

use crate::wire::{ArgumentSource, Callable, Command, Plan, Shape, Target};
use clap::{Arg, ArgAction, ArgGroup, ArgMatches};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

/// Maximum protected text size in bytes.
pub const MAX_PROTECTED_BYTES: usize = 1024 * 1024;

/// Presentation choice, never callable payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    /// Human-oriented output.
    Human,
    /// Structured output, including parser refusals.
    Json,
}

/// Process configuration separate from typed invocation input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// Optional configuration location.
    pub config: Option<PathBuf>,
    /// Optional state-directory location.
    pub state_dir: Option<PathBuf>,
    /// Selected output policy.
    pub output: OutputMode,
}

/// Validated call. Deliberately does not implement Debug because input can be protected.
pub struct Invocation<'a> {
    /// Callable identity from the binding.
    pub callable: &'a str,
    /// Owner-qualified target.
    pub target: &'a Target,
    /// Global process context.
    pub context: Context,
    /// Validated ephemeral payload.
    pub input: Value,
}

/// Application reply, validated before output.
pub enum HandlerReply {
    /// Success value following the declared result contract.
    Success(Value),
    /// A declared error code and its typed data.
    Error {
        /// Declared stable code.
        code: String,
        /// Declared payload.
        data: Value,
    },
    /// A declared, typed application usage failure (for example invalid configuration).
    UsageError {
        /// Declared stable code.
        code: String,
        /// Declared payload, with usage classification chosen by the application.
        data: Value,
    },
    /// No application implementation has been installed.
    Unavailable,
    /// Application cancellation.
    Interrupted,
}

/// Application-owned implementation seam.
pub trait Handler {
    /// Run one validated invocation.
    fn call(&mut self, invocation: &Invocation<'_>) -> HandlerReply;
}

/// Safe default for the generated standalone binary.
pub struct UnavailableHandler;
impl Handler for UnavailableHandler {
    fn call(&mut self, _invocation: &Invocation<'_>) -> HandlerReply {
        HandlerReply::Unavailable
    }
}

/// Selected protected acquisition channel, with no secret argv value variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtectedSource {
    /// Read an admitted file.
    File(PathBuf),
    /// Read standard input.
    Stdin,
    /// Read the controlling terminal with echo disabled.
    HiddenTty,
    /// A nonsecret business document file (no owner-only admission rule).
    DocumentFile(PathBuf),
    /// A nonsecret business document on stdin.
    DocumentStdin,
}

/// Acquisition failures carry no source bytes or underlying error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireError {
    /// Unavailable, unsafe, oversized or malformed source.
    Unavailable,
    /// Interrupted read or user cancellation.
    Interrupted,
}

/// Protected acquisition seam. Implementations must preserve the admission/echo rules.
pub trait Sources {
    /// Acquire at most the bounded UTF-8 input, without echoing it.
    fn acquire(&mut self, source: ProtectedSource) -> Result<String, AcquireError>;
}

/// Native bounded acquisition; protected files require owner-only permissions.
pub struct OsSources;
impl Sources for OsSources {
    fn acquire(&mut self, source: ProtectedSource) -> Result<String, AcquireError> {
        match source {
            ProtectedSource::File(path) => protected_file(&path).and_then(bounded_read),
            ProtectedSource::DocumentFile(path) => {
                #[cfg(unix)]
                let file = {
                    use rustix::fs::{Mode, OFlags};
                    // Admission follows this exact descriptor. A FIFO must not wait for
                    // an external writer before we can discover it is not a regular file.
                    let fd = rustix::fs::open(
                        &path,
                        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                    .map_err(|error| acquire_error(&std::io::Error::from(error)))?;
                    std::fs::File::from(fd)
                };
                #[cfg(not(unix))]
                let file = std::fs::File::open(path).map_err(|error| acquire_error(&error))?;
                if !file
                    .metadata()
                    .map_err(|error| acquire_error(&error))?
                    .is_file()
                {
                    return Err(AcquireError::Unavailable);
                }
                bounded_read(file)
            }
            ProtectedSource::Stdin | ProtectedSource::DocumentStdin => {
                bounded_read(std::io::stdin().lock())
            }
            ProtectedSource::HiddenTty => {
                let value = rpassword::prompt_password("Protected input: ")
                    .map_err(|error| acquire_error(&error))?;
                if value.len() > MAX_PROTECTED_BYTES {
                    Err(AcquireError::Unavailable)
                } else {
                    Ok(value)
                }
            }
        }
    }
}

fn acquire_error(error: &std::io::Error) -> AcquireError {
    if error.kind() == std::io::ErrorKind::Interrupted {
        AcquireError::Interrupted
    } else {
        AcquireError::Unavailable
    }
}

fn bounded_read(reader: impl Read) -> Result<String, AcquireError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_PROTECTED_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| acquire_error(&error))?;
    if bytes.len() > MAX_PROTECTED_BYTES {
        return Err(AcquireError::Unavailable);
    }
    String::from_utf8(bytes).map_err(|_| AcquireError::Unavailable)
}

fn protected_file(path: &std::path::Path) -> Result<std::fs::File, AcquireError> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};
        use std::os::unix::fs::MetadataExt;
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| AcquireError::Unavailable)?;
        let file = std::fs::File::from(fd);
        let metadata = file.metadata().map_err(|error| acquire_error(&error))?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(AcquireError::Unavailable);
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        // This version has no Windows ACL admission implementation.
        Err(AcquireError::Unavailable)
    }
}

/// Which native dynamic value needs validation.
pub enum DynamicPhase {
    /// Native input parsed from the declared JSON text field.
    Input,
    /// Native result, after outer result-type validation.
    Result,
    /// Declared error data, after outer error-type validation.
    Error(String),
}

/// Application-owned native schema resolver/validator. Absence fails closed.
pub trait DynamicValidator {
    /// Resolve the exact operation/schema identity and validate this value.
    fn validate(
        &mut self,
        invocation: &Invocation<'_>,
        phase: DynamicPhase,
        value: &Value,
    ) -> Result<(), DynamicError>;
}

/// Closed native validation failures, without resolver diagnostics or input data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicError {
    /// The value does not satisfy the resolved native schema.
    InvalidValue,
    /// The exact operation/schema authority is unavailable, unresolved or stale.
    Unavailable,
    /// Validation or resolution was interrupted.
    Interrupted,
}

/// Complete process output, without implicitly writing to a stream.
#[derive(Debug, PartialEq, Eq)]
pub struct ProcessOutput {
    /// 0 success, 1 application/internal failure, 2 usage/input failure, 130 interruption.
    pub exit_code: i32,
    /// Success/help bytes.
    pub stdout: String,
    /// Error bytes.
    pub stderr: String,
}

/// Build the generated Clap command tree.
pub fn command(plan: &Plan) -> clap::Command {
    // A global the binding omits is no flag at all: not parsed, not in help, not completed.
    let mut root = clap::Command::new(plan.binary.clone())
        .about(plan.about.clone())
        .subcommand_required(true);
    if let Some(config) = &plan.globals.config {
        root = root.arg(
            Arg::new("global_config")
                .long(config.clone())
                .global(true)
                .value_parser(clap::value_parser!(PathBuf)),
        );
    }
    root = root.arg(
        Arg::new("global_state")
            .long(plan.globals.state.clone())
            .global(true)
            .value_parser(clap::value_parser!(PathBuf)),
    );
    if let Some(output) = &plan.globals.output {
        root = root.arg(
            Arg::new("global_output")
                .long(output.clone())
                .global(true)
                .value_parser(["human", "json"])
                .default_value("human"),
        );
    }
    let mut roots = BTreeMap::new();
    let mut groups = BTreeMap::<String, BTreeMap<String, clap::Command>>::new();
    for declaration in &plan.commands {
        let callable = &plan.callables[&declaration.callable];
        for path in std::iter::once(&declaration.path).chain(&declaration.aliases) {
            let name = path.last().expect("compiled paths are nonempty");
            let leaf = leaf_command(name, declaration, callable);
            if path.len() == 1 {
                roots.insert(name.clone(), leaf);
            } else {
                groups
                    .entry(path[0].clone())
                    .or_default()
                    .insert(name.clone(), leaf);
            }
        }
    }
    for (name, leaves) in groups {
        roots.insert(
            name.clone(),
            clap::Command::new(name)
                .subcommand_required(true)
                .subcommands(leaves.into_values()),
        );
    }
    root = root.subcommands(roots.into_values());
    root.subcommand(
        clap::Command::new("completions")
            .about("Emit shell completions")
            .arg(Arg::new("shell").required(true).value_parser(["bash"])),
    )
}

fn field_shapes(callable: &Callable) -> BTreeMap<String, Shape> {
    match callable.input.as_ref().map(|contract| &contract.shape) {
        Some(Shape::Struct { fields }) => fields.clone(),
        None => BTreeMap::new(),
        _ => unreachable!("the binding compiler requires a struct input"),
    }
}

fn value_arg(id: String) -> Arg {
    Arg::new(id)
        .num_args(1)
        .allow_negative_numbers(true)
        .value_parser(clap::builder::StringValueParser::new())
}

fn leaf_command(name: &str, declaration: &Command, callable: &Callable) -> clap::Command {
    let mut command = clap::Command::new(name.to_owned()).about(declaration.about.clone());
    let fields = field_shapes(callable);
    // Positionals are consecutive from 1, so a trailing list takes the next index.
    let trailing_index = 1 + declaration
        .arguments
        .iter()
        .filter(|a| matches!(a.source, ArgumentSource::Positional { .. }))
        .count();
    for argument in &declaration.arguments {
        let field = &argument.field;
        let required = !fields[field].optional();
        match &argument.source {
            ArgumentSource::Option { long } => {
                command = command.arg(
                    value_arg(format!("field:{field}"))
                        .long(long.clone())
                        .required(required),
                );
            }
            ArgumentSource::Positional { index } => {
                command = command.arg(
                    value_arg(format!("field:{field}"))
                        .index(*index)
                        .required(required),
                );
            }
            ArgumentSource::Protected {
                file,
                stdin,
                hidden_tty,
            } => {
                let ids = [
                    format!("source:{field}:file"),
                    format!("source:{field}:stdin"),
                    format!("source:{field}:tty"),
                ];
                command = command
                    .arg(value_arg(ids[0].clone()).long(file.clone()))
                    .arg(
                        Arg::new(ids[1].clone())
                            .long(stdin.clone())
                            .action(ArgAction::SetTrue),
                    )
                    .arg(
                        Arg::new(ids[2].clone())
                            .long(hidden_tty.clone())
                            .action(ArgAction::SetTrue),
                    )
                    .group(
                        ArgGroup::new(format!("sources:{field}"))
                            .args(ids)
                            .required(required)
                            .multiple(false),
                    );
            }
            ArgumentSource::Document {
                inline,
                file,
                stdin,
            } => {
                let ids = [
                    format!("source:{field}:inline"),
                    format!("source:{field}:file"),
                    format!("source:{field}:stdin"),
                ];
                command = command
                    .arg(value_arg(ids[0].clone()).long(inline.clone()))
                    .arg(value_arg(ids[1].clone()).long(file.clone()))
                    .arg(
                        Arg::new(ids[2].clone())
                            .long(stdin.clone())
                            .action(ArgAction::SetTrue),
                    )
                    .group(
                        ArgGroup::new(format!("sources:{field}"))
                            .args(ids)
                            .required(required)
                            .multiple(false),
                    );
            }
            // Reachable only after the first `--`; a later `--` is a value.
            ArgumentSource::Trailing {} => {
                command = command.arg(
                    Arg::new(format!("field:{field}"))
                        .index(trailing_index)
                        .last(true)
                        .num_args(1..)
                        .action(ArgAction::Append)
                        .value_parser(clap::builder::StringValueParser::new()),
                );
            }
        }
    }
    command
}

/// Whether the arguments select JSON before they are parsed. A CLI without an output flag writes
/// JSON only, so it always does.
fn selected_json(plan: &Plan, args: &[OsString]) -> bool {
    let Some(output) = &plan.globals.output else {
        return true;
    };
    let flag = format!("--{output}");
    let equals = format!("{flag}=json");
    let mut previous = false;
    for arg in args.iter().skip(1) {
        if arg == "--" {
            break;
        }
        if arg == equals.as_str() || (previous && arg == "json") {
            return true;
        }
        previous = arg == flag.as_str();
    }
    false
}

fn failure(output: OutputMode, exit_code: i32, code: &str, data: &Value) -> ProcessOutput {
    let stderr = if output == OutputMode::Json {
        format!(
            "{}\n",
            json!({"ok":false,"error":{"code":code,"data":data}})
        )
    } else if data == &json!({}) {
        format!("{code}\n")
    } else {
        format!("{code}: {data}\n")
    };
    ProcessOutput {
        exit_code,
        stdout: String::new(),
        stderr,
    }
}

fn internal(output: OutputMode, exit_code: i32, code: &str) -> ProcessOutput {
    failure(output, exit_code, code, &json!({}))
}

fn acquired(
    source: ProtectedSource,
    sources: &mut dyn Sources,
    output: OutputMode,
) -> Result<String, ProcessOutput> {
    let value = sources.acquire(source).map_err(|error| match error {
        AcquireError::Unavailable => internal(output, 2, "cli_source"),
        AcquireError::Interrupted => internal(output, 130, "cli_interrupted"),
    })?;
    if value.len() > MAX_PROTECTED_BYTES {
        return Err(internal(output, 2, "cli_source"));
    }
    Ok(value)
}

fn argument_text(
    source: &ArgumentSource,
    field: &str,
    matches: &ArgMatches,
    sources: &mut dyn Sources,
    output: OutputMode,
) -> Result<Option<String>, ProcessOutput> {
    match source {
        ArgumentSource::Option { .. } | ArgumentSource::Positional { .. } => Ok(matches
            .get_one::<String>(&format!("field:{field}"))
            .cloned()),
        ArgumentSource::Protected { .. } => {
            let selected =
                if let Some(path) = matches.get_one::<String>(&format!("source:{field}:file")) {
                    Some(ProtectedSource::File(path.into()))
                } else if matches.get_flag(&format!("source:{field}:stdin")) {
                    Some(ProtectedSource::Stdin)
                } else if matches.get_flag(&format!("source:{field}:tty")) {
                    Some(ProtectedSource::HiddenTty)
                } else {
                    None
                };
            selected.map(|s| acquired(s, sources, output)).transpose()
        }
        ArgumentSource::Document { .. } => {
            if let Some(text) = matches.get_one::<String>(&format!("source:{field}:inline")) {
                if text.len() > MAX_PROTECTED_BYTES {
                    return Err(internal(output, 2, "cli_source"));
                }
                return Ok(Some(text.clone()));
            }
            let selected =
                if let Some(path) = matches.get_one::<String>(&format!("source:{field}:file")) {
                    Some(ProtectedSource::DocumentFile(path.into()))
                } else if matches.get_flag(&format!("source:{field}:stdin")) {
                    Some(ProtectedSource::DocumentStdin)
                } else {
                    None
                };
            selected.map(|s| acquired(s, sources, output)).transpose()
        }
        ArgumentSource::Trailing {} => unreachable!("payload reads a trailing list as words"),
    }
}

/// The words after `--`, verbatim; `[]` when there are none.
fn trailing_words(field: &str, matches: &ArgMatches) -> Value {
    Value::Array(
        matches
            .get_many::<String>(&format!("field:{field}"))
            .into_iter()
            .flatten()
            .cloned()
            .map(Value::String)
            .collect(),
    )
}

fn json_argument(text: &str) -> Result<Value, serde_json::Error> {
    // serde_json retains -0 as a floating-point value. Preserve the documented
    // integer grammar by normalizing only that complete token, outside strings.
    // Fractions, exponents and malformed JSON still go through its normal reader.
    let bytes = text.as_bytes();
    let mut normalized: Option<Vec<u8>> = None;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'-'
            && bytes.get(index + 1) == Some(&b'0')
            && (index == 0
                || matches!(bytes[index - 1], b'[' | b'{' | b':' | b',')
                || bytes[index - 1].is_ascii_whitespace())
            && bytes
                .get(index + 2)
                .is_none_or(|next| matches!(next, b']' | b'}' | b',') || next.is_ascii_whitespace())
        {
            // Replacing an ASCII sign with whitespace retains every string byte.
            normalized.get_or_insert_with(|| bytes.to_vec())[index] = b' ';
        }
    }
    serde_json::from_slice(normalized.as_deref().unwrap_or(bytes))
}

/// Parse dynamic payload text, refusing a duplicate object key at any depth. `serde_json` keeps
/// the last of two equal keys, so the validator would otherwise check a value other than the
/// text the handler receives.
fn native_json(text: &str) -> Option<Value> {
    serde_json::from_str::<UniqueKeys>(text).ok()?;
    serde_json::from_str(text).ok()
}

/// A JSON document read only to prove that no object repeats a key.
struct UniqueKeys;

impl<'de> serde::Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueKeys)
    }
}

impl<'de> serde::de::Visitor<'de> for UniqueKeys {
    type Value = Self;
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_str<E>(self, _: &str) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_unit<E>(self) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut items: A) -> Result<Self, A::Error> {
        while items.next_element::<Self>()?.is_some() {}
        Ok(self)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut entries: A) -> Result<Self, A::Error> {
        let mut keys = std::collections::BTreeSet::new();
        while let Some(key) = entries.next_key::<String>()? {
            if !keys.insert(key) {
                // The message names no key: nothing of the input reaches an error.
                return Err(serde::de::Error::custom("duplicate object key"));
            }
            entries.next_value::<Self>()?;
        }
        Ok(self)
    }
}

/// Every invalid input answers the callable's declared `invalid_input` error with `{}` data, or
/// the adapter's closed code when the binding declares none. Neither carries the input text.
fn invalid(callable: &Callable, output: OutputMode, adapter_code: &str) -> ProcessOutput {
    internal(
        output,
        2,
        callable.invalid_input.as_deref().unwrap_or(adapter_code),
    )
}

fn payload(
    declaration: &Command,
    callable: &Callable,
    matches: &ArgMatches,
    sources: &mut dyn Sources,
    output: OutputMode,
) -> Result<Value, ProcessOutput> {
    let fields = field_shapes(callable);
    let mut object = Map::new();
    // Validate ordinary values before acquiring protected ones.
    let mut arguments = declaration.arguments.iter().collect::<Vec<_>>();
    arguments.sort_by_key(|a| matches!(a.source, ArgumentSource::Protected { .. }));
    for argument in arguments {
        let shape = &fields[&argument.field];
        if matches!(argument.source, ArgumentSource::Trailing {}) {
            object.insert(
                argument.field.clone(),
                trailing_words(&argument.field, matches),
            );
            continue;
        }
        let Some(text) =
            argument_text(&argument.source, &argument.field, matches, sources, output)?
        else {
            continue;
        };
        let value = if matches!(shape.required(), Shape::String | Shape::Enum { .. }) {
            Value::String(text)
        } else {
            json_argument(&text).map_err(|_| invalid(callable, output, "cli_input"))?
        };
        if !shape.accepts(&value) {
            return Err(invalid(callable, output, "cli_input"));
        }
        object.insert(argument.field.clone(), value);
    }
    let value = Value::Object(object);
    if callable
        .input
        .as_ref()
        .is_some_and(|input| !input.shape.accepts(&value))
    {
        return Err(invalid(callable, output, "cli_input"));
    }
    Ok(value)
}

/// The process context the parsed globals select. A CLI without an output flag writes JSON only.
fn context(plan: &Plan, matches: &ArgMatches) -> Context {
    let output = if plan.globals.output.is_none()
        || matches
            .get_one::<String>("global_output")
            .is_some_and(|v| v == "json")
    {
        OutputMode::Json
    } else {
        OutputMode::Human
    };
    Context {
        // An undefined argument cannot be asked for: clap panics on an unknown id.
        config: plan
            .globals
            .config
            .as_ref()
            .and_then(|_| matches.get_one::<PathBuf>("global_config").cloned()),
        state_dir: matches.get_one::<PathBuf>("global_state").cloned(),
        output,
    }
}

/// Parse, acquire, validate, invoke once and validate/render the reply.
pub fn run(
    plan: &Plan,
    args: Vec<OsString>,
    sources: &mut dyn Sources,
    handler: &mut dyn Handler,
    mut dynamic: Option<&mut dyn DynamicValidator>,
) -> ProcessOutput {
    let preliminary = if selected_json(plan, &args) {
        OutputMode::Json
    } else {
        OutputMode::Human
    };
    let matches = match command(plan).try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            return if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                ProcessOutput {
                    exit_code: 0,
                    stdout: error.to_string(),
                    stderr: String::new(),
                }
            } else {
                internal(preliminary, 2, "cli_parse")
            };
        }
    };
    let context = context(plan, &matches);
    let output = context.output;
    let mut path = Vec::new();
    let mut leaf = &matches;
    while let Some((name, next)) = leaf.subcommand() {
        path.push(name.to_owned());
        leaf = next;
    }
    if path == ["completions"] {
        let mut bytes = Vec::new();
        clap_complete::generate(
            clap_complete::Shell::Bash,
            &mut command(plan),
            &plan.binary,
            &mut bytes,
        );
        return ProcessOutput {
            exit_code: 0,
            stdout: String::from_utf8(bytes).expect("bash completion is UTF-8"),
            stderr: String::new(),
        };
    }
    let declaration = plan
        .commands
        .iter()
        .find(|c| c.path == path || c.aliases.contains(&path))
        .expect("compiled command path");
    let callable = &plan.callables[&declaration.callable];
    let input = match payload(declaration, callable, leaf, sources, output) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let invocation = Invocation {
        callable: &declaration.callable,
        target: &callable.target,
        context,
        input,
    };
    let is_dynamic = if let Target::Dynamic { payload_field, .. } = &callable.target {
        let Some(validator) = dynamic.as_mut() else {
            return internal(output, 1, "cli_dynamic_validator_unavailable");
        };
        let Some(native) = native_json(
            invocation.input[payload_field]
                .as_str()
                .expect("typed dynamic field"),
        ) else {
            return invalid(callable, output, "cli_dynamic_input");
        };
        match validator.validate(&invocation, DynamicPhase::Input, &native) {
            Ok(()) => {}
            Err(DynamicError::InvalidValue) => {
                return invalid(callable, output, "cli_dynamic_input")
            }
            Err(error) => return dynamic_failure(output, error, 2, "cli_dynamic_input"),
        }
        true
    } else {
        false
    };
    render_reply(
        callable,
        &invocation,
        handler.call(&invocation),
        if is_dynamic { dynamic } else { None },
    )
}

fn render_reply(
    callable: &Callable,
    invocation: &Invocation<'_>,
    reply: HandlerReply,
    mut dynamic: Option<&mut dyn DynamicValidator>,
) -> ProcessOutput {
    let output = invocation.context.output;
    let error_exit = if matches!(reply, HandlerReply::UsageError { .. }) {
        2
    } else {
        1
    };
    match reply {
        HandlerReply::Success(result) => {
            if !callable.result.shape.accepts(&result) {
                return internal(output, 1, "cli_result");
            }
            if let Some(validator) = dynamic.as_mut() {
                if let Err(error) = validator.validate(invocation, DynamicPhase::Result, &result) {
                    return dynamic_failure(output, error, 1, "cli_dynamic_result");
                }
            }
            let stdout = if output == OutputMode::Json {
                format!("{}\n", json!({"ok":true,"result":result}))
            } else {
                format!("{result}\n")
            };
            ProcessOutput {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
        HandlerReply::Error { code, data } | HandlerReply::UsageError { code, data } => {
            if !callable
                .errors
                .get(&code)
                .is_some_and(|contract| contract.shape.accepts(&data))
            {
                return internal(output, 1, "cli_error");
            }
            if let Some(validator) = dynamic.as_mut() {
                if let Err(error) =
                    validator.validate(invocation, DynamicPhase::Error(code.clone()), &data)
                {
                    return dynamic_failure(output, error, 1, "cli_dynamic_error");
                }
            }
            failure(output, error_exit, &code, &data)
        }
        HandlerReply::Unavailable => internal(output, 1, "cli_handler_unavailable"),
        HandlerReply::Interrupted => internal(output, 130, "cli_interrupted"),
    }
}

fn dynamic_failure(
    output: OutputMode,
    error: DynamicError,
    invalid_exit: i32,
    invalid_code: &str,
) -> ProcessOutput {
    match error {
        DynamicError::InvalidValue => internal(output, invalid_exit, invalid_code),
        DynamicError::Unavailable => internal(output, 1, "cli_dynamic_validator_unavailable"),
        DynamicError::Interrupted => internal(output, 130, "cli_interrupted"),
    }
}
