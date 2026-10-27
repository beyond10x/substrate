//! The `oci` adapter: one container per run under Docker or podman, the image by digest only
//! (atlas ADR 0109 decisions 3 and 5, ADR 0044 decision 8).
//!
//! It is the `sandbox` repository's `docker run` posture behind the confinement contract, plus
//! what `sandbox` lacks. Every run is
//!
//! | flag | why |
//! |---|---|
//! | `run --rm --name <run> --label …` | one container per run, removed at its end and findable by label |
//! | `--log-driver none` | the runtime keeps no unbounded copy of the output; the adapter drains and bounds it |
//! | `--user <uid>:<gid>` | the embedding process's own identity, so the binds grant nothing it lacks |
//! | `--network none` | no network at all: `network_egress` and `network_ingress` are not served |
//! | `--cap-drop ALL`, `--security-opt no-new-privileges` | no capability, no gain through exec |
//! | `--read-only --tmpfs /tmp` | the image's root is read-only; `/tmp` is private |
//! | `--pids-limit`, `--memory`, `--memory-swap`, `--cpus` | the profile's bounds, when `limits` is served |
//! | `-v <workspace>:/workspace` | the one writable workspace, where the run starts |
//! | `-v <root>:<mount>:ro` | read-only roots and files |
//! | `-v <declared>:<mount>` | operator-declared writable roots |
//! | `-w`, five baseline `-e` variables, then the command's | the start directory and the whole environment |
//! | `--entrypoint "" -- <name>@sha256:<64 hex>` | the image by digest, the command as given |
//!
//! The runtime's default seccomp profile is kept: no flag here touches it, and `open` refuses a
//! runtime whose `info` does not report seccomp. `open` probes the runtime (the program, `info`
//! answering, seccomp, the cgroup controllers behind `limits`, rootless or not) and fails closed as
//! `confinement.adapter-unserved`. `limits` is served only when the runtime reports memory, swap,
//! pids and CPU quota control, so a bound is never silently discarded (invariant 3).
//!
//! It serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
//! `filesystem_view`, `limits` and `audit`, and proves `image_digest_pinned`. Whether the runtime
//! is rootless has no `Fact` variant; [`probe`] reports it.
//!
//! The runtime's socket is root-equivalent on a rootful host
//! (`docs/design/04-security-and-isolation.md`). The adapter's authority is the operator's runtime;
//! a profile reaches it only through the fixed flags above, every value a separate argument, and
//! the image after `--`.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, ReadOnlyFile,
    ReadOnlyRoot, SandboxProfile, Signal, WireValidationError, WorkspaceAccess, WritableRoot,
};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::watch;

use super::{
    AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, ProcessBounds, RenderedCommand, RunExit, RunId,
    RunObservation, RunOutput, RunState,
};
use crate::{DeclaredWritableRoot, DriverError};

/// The label every `oci` container carries, valued `oci`.
pub const LABEL: &str = "dev.b10x.substrate.confinement";
/// The label naming the run a container belongs to.
pub const RUN_LABEL: &str = "dev.b10x.substrate.confinement.run";
/// Each output stream is kept up to this many bytes and drained past it, unless the profile's
/// `io_bounds` names a smaller bound.
pub const OUTPUT_LIMIT_BYTES: u64 = 1_048_576;
/// Runs one adapter tracks; past it, ended runs are forgotten oldest first.
pub const MAX_TRACKED_RUNS: usize = 128;
/// The smallest memory bound a container runtime accepts.
pub const MIN_MEMORY_BYTES: u64 = 6 * 1024 * 1024;
/// The bounds a run gets when its profile states none.
pub const DEFAULT_BOUNDS: ProcessBounds = ProcessBounds {
    timeout_ms: 600_000,
    processes: 256,
    memory_bytes: 1 << 30,
    cpu_millis: 600_000,
};
/// A host path or mount the runtime's `-v` syntax cannot carry: it holds a `:`.
pub const PATH_UNREPRESENTABLE: &str = "confinement.path-unrepresentable";

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const RUNTIME_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const SIGNAL_RETRY: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_millis(100);
const MAX_INFO_BYTES: u64 = 4 << 20;
const MAX_TIMEOUT_MS: u64 = 86_400_000;
const MAX_PROCESSES: u32 = 4_096;
const MAX_MEMORY_BYTES: u64 = 1 << 40;
/// The smallest CPU share a runtime accepts, in thousandths of a CPU.
const MIN_MILLICPUS: u64 = 10;
const BASELINE_ENV: [(&str, &str); 5] = [
    ("PATH", "/usr/bin:/bin"),
    ("HOME", "/tmp"),
    ("LANG", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
    ("TZ", "UTC"),
];

/// What opening `oci` takes.
#[derive(Debug, Clone)]
pub struct OciRequest {
    /// The container runtime program: an absolute path, or a bare name (`docker`, `podman`)
    /// looked up on `PATH`.
    pub runtime: PathBuf,
    /// The writable host roots the operator declared (ADR 0038 kind 1). A profile names one by
    /// name; a name not declared here is `exec.writable-roots-host-unserved`.
    pub writable_roots: Vec<DeclaredWritableRoot>,
}

/// Which runtime answered `info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeFlavor {
    Docker,
    Podman,
}

/// What `open` read from the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProbe {
    /// The program every runtime call runs.
    pub program: PathBuf,
    pub flavor: RuntimeFlavor,
    pub version: String,
    /// The runtime runs without root on the host.
    pub rootless: bool,
    /// Memory, swap, pids and CPU quota control are all reported.
    pub limits: bool,
    /// The CPUs the runtime reports; `--cpus` never exceeds them.
    pub cpus: u32,
}

fn unserved(detail: impl Into<String>) -> ConfinementError {
    ConfinementError::AdapterUnserved {
        adapter: AdapterName::Oci,
        detail: detail.into(),
    }
}

/// Probes the runtime `request` names, as `open` does, without opening the adapter.
///
/// # Errors
///
/// `confinement.adapter-unserved` when the program is absent, `info` does not answer within its
/// bound, or the runtime does not report its default seccomp profile.
pub fn probe(request: &OciRequest) -> Result<RuntimeProbe, ConfinementError> {
    let program = resolve(&request.runtime).ok_or_else(|| {
        unserved(format!(
            "no container runtime program at {}",
            request.runtime.display()
        ))
    })?;
    let info = run_bounded(&program, &["info", "--format", "{{json .}}"], PROBE_TIMEOUT)
        .ok_or_else(|| unserved("the container runtime's info did not answer"))?;
    parse_info(program, &info).map_err(unserved)
}

fn executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn resolve(runtime: &Path) -> Option<PathBuf> {
    if runtime.is_absolute() {
        return executable(runtime).then(|| runtime.to_path_buf());
    }
    if runtime.components().count() != 1 {
        return None;
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(runtime))
        .find(|candidate| executable(candidate))
}

/// Runs a runtime call to completion within `timeout`, returning its stdout when it succeeded.
fn run_bounded(program: &Path, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(MAX_INFO_BYTES).read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().ok()?;
    status
        .filter(std::process::ExitStatus::success)
        .map(|_| out)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

fn cpus(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .and_then(|cpus| u32::try_from(cpus).ok())
        .unwrap_or(0)
}

fn parse_info(program: PathBuf, bytes: &[u8]) -> Result<RuntimeProbe, String> {
    let info: Value = serde_json::from_slice(bytes)
        .map_err(|_| "the container runtime's info is not JSON".to_owned())?;
    let (flavor, version, seccomp, rootless, limits, cpus) = if let Some(host) = info.get("host") {
        let security = host.get("security").cloned().unwrap_or(Value::Null);
        let controllers: Vec<&str> = host
            .get("cgroupControllers")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        (
            RuntimeFlavor::Podman,
            info.pointer("/version/Version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            flag(&security, "seccompEnabled"),
            flag(&security, "rootless"),
            ["cpu", "memory", "pids"]
                .iter()
                .all(|controller| controllers.contains(controller)),
            cpus(host.get("cpus")),
        )
    } else {
        let version = info
            .get("ServerVersion")
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
            .ok_or_else(|| "the container runtime's info names no server".to_owned())?;
        let options: Vec<&str> = info
            .get("SecurityOptions")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        (
            RuntimeFlavor::Docker,
            version.to_owned(),
            options.iter().any(|option| {
                option.split(',').any(|part| part == "name=seccomp")
                    && !option.split(',').any(|part| part == "profile=unconfined")
            }),
            options.contains(&"name=rootless"),
            ["MemoryLimit", "SwapLimit", "PidsLimit", "CpuCfsQuota"]
                .iter()
                .all(|key| flag(&info, key)),
            cpus(info.get("NCPU")),
        )
    };
    if !seccomp {
        return Err("the container runtime does not report its default seccomp profile".to_owned());
    }
    if cpus == 0 {
        return Err("the container runtime reports no CPU".to_owned());
    }
    Ok(RuntimeProbe {
        program,
        flavor,
        version,
        rootless,
        limits,
        cpus,
    })
}

/// The families and facts an opened runtime proves.
pub fn proof(runtime: &RuntimeProbe) -> AdapterFacts {
    AdapterFacts::new(
        AdapterName::Oci,
        None,
        &[
            (ContractFamily::ProcessRun, true),
            (ContractFamily::ProcessWatch, true),
            (ContractFamily::OutputRead, true),
            (ContractFamily::Wait, true),
            (ContractFamily::Workspace, true),
            (ContractFamily::FilesystemView, true),
            (ContractFamily::Limits, runtime.limits),
            (ContractFamily::Audit, true),
        ],
        &[(Fact::ImageDigestPinned, true)],
    )
}

fn check_declared(roots: &[DeclaredWritableRoot]) -> Result<(), ConfinementError> {
    for (index, root) in roots.iter().enumerate() {
        let valid = substrate_wire::is_root_name(&root.name)
            && !roots[..index].iter().any(|other| other.name == root.name)
            && root.path.is_absolute()
            && !root.path.as_os_str().as_encoded_bytes().contains(&b':');
        if !valid {
            return Err(DriverError::refused(
                "config.writable-root-invalid",
                format!(
                    "Writable host root {:?} is not a unique root name with an absolute path \
                     the runtime can bind.",
                    root.name
                ),
                "config.writable_roots",
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn open(request: OciRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    check_declared(&request.writable_roots)?;
    let runtime = probe(&request)?;
    Ok(Arc::new(OciAdapter {
        facts: proof(&runtime),
        runtime,
        writable_roots: request.writable_roots,
        identity: (
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw(),
        ),
        runs: Mutex::new(Vec::new()),
    }))
}

/// Whether the name before `@sha256:` is a plain image reference: it starts with a lowercase
/// letter or digit and holds only lowercase letters, digits, `.`, `_`, `-`, `/` and `:`. Nothing
/// that a runtime could read as a flag reaches its command line.
fn reference_is_plain(image: &str) -> bool {
    let Some((name, _)) = image.rsplit_once("@sha256:") else {
        return false;
    };
    name.bytes()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-/:".contains(&byte)
        })
}

fn representable(path: &str, field: &str) -> Result<(), ConfinementError> {
    if path.contains(':') {
        return Err(DriverError::refused(
            PATH_UNREPRESENTABLE,
            "A path holds a ':', which the container runtime's bind syntax cannot carry.",
            field,
        )
        .into());
    }
    Ok(())
}

fn limit_unserved(message: &str) -> ConfinementError {
    DriverError::exhausted("exec.limit-unserved", message, "limits").into()
}

/// What one run is bounded by.
#[derive(Debug, Clone, Copy)]
struct RunBounds {
    timeout: Duration,
    output_bytes: u64,
    /// `--pids-limit`, `--memory` and `--memory-swap`, and `--cpus` in thousandths, when served.
    limits: Option<(u32, u64, u64)>,
}

/// One bind, as the runtime's `-v` value.
fn bind(host: &str, mount: &str, read_only: bool) -> String {
    if read_only {
        format!("{host}:{mount}:ro")
    } else {
        format!("{host}:{mount}")
    }
}

fn refused(code: &'static str, message: impl Into<String>, field: &str) -> ConfinementError {
    DriverError::refused(code, message, field).into()
}

fn wire_refusal(refusal: substrate_wire::ExecStartRefusal, message: String) -> ConfinementError {
    if refusal.status == 501 {
        DriverError::unserved(refusal.code, message, "writable_roots").into()
    } else {
        DriverError::refused(refusal.code, message, "writable_roots").into()
    }
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

struct OciRun {
    id: RunId,
    phase: watch::Receiver<Phase>,
    captured: Arc<Mutex<Captured>>,
    /// The last signal the adapter sent the container, read when the run ends.
    signalled: Arc<Mutex<Option<i32>>>,
}

pub struct OciAdapter {
    facts: AdapterFacts,
    runtime: RuntimeProbe,
    writable_roots: Vec<DeclaredWritableRoot>,
    identity: (u32, u32),
    runs: Mutex<Vec<Arc<OciRun>>>,
}

impl OciAdapter {
    fn run(&self, id: &RunId) -> Result<Arc<OciRun>, ConfinementError> {
        self.runs
            .lock()
            .iter()
            .find(|run| &run.id == id)
            .cloned()
            .ok_or_else(super::unknown_run)
    }

    fn observation(run: &OciRun) -> RunObservation {
        let (state, exit) = match *run.phase.borrow() {
            Phase::Running => (RunState::Running, None),
            Phase::Ended(exit) => (RunState::Ended, Some(exit)),
            Phase::Unknown => (RunState::Unknown, None),
        };
        RunObservation {
            run: run.id.clone(),
            adapter: AdapterName::Oci,
            inner: None,
            declared_insecure: None,
            state,
            exit,
        }
    }

    fn reserve(&self) -> Result<(), ConfinementError> {
        let mut runs = self.runs.lock();
        while runs.len() >= MAX_TRACKED_RUNS {
            let Some(ended) = runs
                .iter()
                .position(|run| !matches!(*run.phase.borrow(), Phase::Running))
            else {
                return Err(DriverError::exhausted(
                    "exec.tracking-capacity",
                    "Every tracked oci run is still running.",
                    "run",
                )
                .into());
            };
            runs.remove(ended);
        }
        Ok(())
    }

    /// The profile's bounds, or the defaults, checked against what a runtime accepts.
    fn bounds(&self, profile: &ConfinementProfile) -> Result<RunBounds, ConfinementError> {
        let bounds = profile.process_bounds.unwrap_or(DEFAULT_BOUNDS);
        if !(1..=MAX_TIMEOUT_MS).contains(&bounds.timeout_ms)
            || !(1..=MAX_PROCESSES).contains(&bounds.processes)
            || !(MIN_MEMORY_BYTES..=MAX_MEMORY_BYTES).contains(&bounds.memory_bytes)
            || !(1..=MAX_TIMEOUT_MS).contains(&bounds.cpu_millis)
        {
            return Err(limit_unserved(
                "The bounds are outside what the container runtime accepts.",
            ));
        }
        let output_bytes = profile
            .io_bounds
            .map_or(OUTPUT_LIMIT_BYTES, |io| io.output_bytes);
        if !(1..=OUTPUT_LIMIT_BYTES).contains(&output_bytes) {
            return Err(limit_unserved(
                "The output bound is outside what the oci adapter keeps.",
            ));
        }
        // A rate that spends at most `cpu_millis` over the wall bound: the cumulative budget holds
        // whatever the run does, and it is never raised past the CPUs the runtime has.
        let millicpus = (bounds.cpu_millis.saturating_mul(1_000) / bounds.timeout_ms)
            .min(u64::from(self.runtime.cpus).saturating_mul(1_000));
        if millicpus < MIN_MILLICPUS {
            return Err(limit_unserved(
                "The CPU budget over the wall bound is below the runtime's smallest CPU share.",
            ));
        }
        Ok(RunBounds {
            timeout: Duration::from_millis(bounds.timeout_ms),
            output_bytes,
            limits: self.facts.serves(&ContractFamily::Limits).then_some((
                bounds.processes,
                bounds.memory_bytes,
                millicpus,
            )),
        })
    }

    /// Every rule on the profile's mounts that needs no I/O.
    fn check_mounts(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
        substrate_wire::validate_read_only_roots(&profile.read_only_roots).map_err(|error| {
            refused(
                substrate_wire::READ_ONLY_ROOT_INVALID,
                substrate_wire::read_only_root_invalid_message(&error),
                "read_only_roots",
            )
        })?;
        substrate_wire::validate_read_only_files(
            &profile.read_only_files,
            &profile.read_only_roots,
        )
        .map_err(|error| {
            refused(
                substrate_wire::read_only_file_refusal(&error)
                    .unwrap_or(substrate_wire::READ_ONLY_FILE_PATH_INVALID),
                error.to_string(),
                "read_only_files",
            )
        })?;
        let input = writable_input(profile);
        let refusal = |error: &WireValidationError| {
            substrate_wire::writable_root_refusal(error).map_or_else(
                || {
                    refused(
                        substrate_wire::EXEC_WRITABLE_ROOT_MOUNT_INVALID,
                        error.to_string(),
                        "writable_roots",
                    )
                },
                |(refusal, message)| wire_refusal(refusal, message),
            )
        };
        substrate_wire::validate_writable_roots(&input).map_err(|error| refusal(&error))?;
        for root in &profile.writable_roots {
            if !self
                .writable_roots
                .iter()
                .any(|declared| declared.name == root.host)
            {
                return Err(refusal(&WireValidationError::WritableRootHostUnserved {
                    name: root.host.clone(),
                }));
            }
        }
        for root in &profile.read_only_roots {
            representable(&root.host_path, "read_only_roots")?;
            representable(&root.mount, "read_only_roots")?;
        }
        for file in &profile.read_only_files {
            representable(&file.host_path, "read_only_files")?;
            representable(&file.mount, "read_only_files")?;
        }
        for root in &profile.writable_roots {
            representable(&root.mount, "writable_roots")?;
        }
        Ok(())
    }

    /// The `-v` values and the start directory, each host path checked now.
    fn mounts(
        &self,
        admitted: &AdmittedProfile,
    ) -> Result<(Vec<String>, String), ConfinementError> {
        let profile = admitted.profile();
        let workspace = super::workspace_of(admitted)?;
        let text = workspace.to_str().ok_or_else(|| {
            refused(
                "workspace.path-escape",
                "The workspace path is not UTF-8.",
                "workspace",
            )
        })?;
        representable(text, "workspace")?;
        if !workspace.is_absolute()
            || std::fs::canonicalize(workspace).ok().as_ref() != Some(workspace)
        {
            if !workspace.exists() {
                return Err(DriverError::not_found().into());
            }
            return Err(refused(
                "workspace.path-escape",
                "The oci workspace must be an absolute path reached without a symlink.",
                "workspace",
            ));
        }
        if !workspace.is_dir() {
            return Err(DriverError::not_found().into());
        }
        let mut binds = vec![bind(text, substrate_wire::EXEC_WORKSPACE_MOUNT, false)];
        let mut directory = substrate_wire::EXEC_WORKSPACE_MOUNT.to_owned();
        if let Some(workdir) = &profile.workdir {
            let relative = super::relative_workdir(workdir)?;
            let host = workspace.join(&relative);
            if std::fs::canonicalize(&host).ok().as_ref() != Some(&host) || !host.is_dir() {
                return Err(refused(
                    substrate_wire::EXEC_WORKDIR_MISSING,
                    "The working directory is not a directory beneath the workspace.",
                    "workdir",
                ));
            }
            directory = format!("{directory}/{}", relative.display());
        }
        for root in &profile.read_only_roots {
            check_root(root)?;
            binds.push(bind(&root.host_path, &root.mount, true));
        }
        for file in &profile.read_only_files {
            check_file(file)?;
            binds.push(bind(&file.host_path, &file.mount, true));
        }
        for root in &profile.writable_roots {
            let declared = self
                .writable_roots
                .iter()
                .find(|declared| declared.name == root.host)
                .ok_or_else(|| {
                    refused(
                        substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
                        "The writable host root is not declared.",
                        "writable_roots",
                    )
                })?;
            self.check_writable(declared)?;
            let host = declared.path.to_str().ok_or_else(|| {
                refused(
                    substrate_wire::EXEC_WRITABLE_ROOT_NOT_A_DIRECTORY,
                    "The writable host root's path is not UTF-8.",
                    "writable_roots",
                )
            })?;
            binds.push(bind(host, &root.mount, false));
        }
        Ok((binds, directory))
    }

    fn check_writable(&self, declared: &DeclaredWritableRoot) -> Result<(), ConfinementError> {
        let name = &declared.name;
        let Ok(metadata) = std::fs::symlink_metadata(&declared.path) else {
            return Err(refused(
                substrate_wire::EXEC_WRITABLE_ROOT_ABSENT,
                format!("Writable host root {name} is not present on this host."),
                "writable_roots",
            ));
        };
        if !metadata.is_dir()
            || std::fs::canonicalize(&declared.path).ok().as_ref() != Some(&declared.path)
        {
            return Err(refused(
                substrate_wire::EXEC_WRITABLE_ROOT_NOT_A_DIRECTORY,
                format!("Writable host root {name} is not a directory reached without a symlink."),
                "writable_roots",
            ));
        }
        if metadata.uid() != self.identity.0 {
            return Err(refused(
                substrate_wire::EXEC_WRITABLE_ROOT_NOT_OWNED,
                format!("Writable host root {name} is not owned by the run's user."),
                "writable_roots",
            ));
        }
        Ok(())
    }

    /// Everything one start or render needs, checked: the runtime's arguments and the bounds.
    fn prepare(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
        run: &RunId,
    ) -> Result<(Vec<String>, RunBounds), ConfinementError> {
        super::check_command(command)?;
        check_environment(command)?;
        let profile = admitted.profile();
        let image = profile
            .image
            .as_deref()
            .filter(|image| super::image_is_pinned(image) && reference_is_plain(image))
            .ok_or(ConfinementError::ImageUnpinned {
                adapter: AdapterName::Oci,
            })?;
        let bounds = self.bounds(profile)?;
        let (binds, directory) = self.mounts(admitted)?;
        let mut args: Vec<String> = [
            "run",
            "--rm",
            "--name",
            run.as_str(),
            "--label",
            &format!("{LABEL}=oci"),
            "--label",
            &format!("{RUN_LABEL}={}", run.as_str()),
            "--log-driver",
            "none",
        ]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
        if self.runtime.flavor == RuntimeFlavor::Podman && self.runtime.rootless {
            args.push("--userns=keep-id".to_owned());
        }
        let (uid, gid) = self.identity;
        args.extend([
            "--user".to_owned(),
            format!("{uid}:{gid}"),
            "--network".to_owned(),
            "none".to_owned(),
            "--cap-drop".to_owned(),
            "ALL".to_owned(),
            "--security-opt".to_owned(),
            "no-new-privileges".to_owned(),
            "--read-only".to_owned(),
            "--tmpfs".to_owned(),
            "/tmp".to_owned(),
        ]);
        if let Some((processes, memory, millicpus)) = bounds.limits {
            args.extend([
                "--pids-limit".to_owned(),
                processes.to_string(),
                "--memory".to_owned(),
                memory.to_string(),
                "--memory-swap".to_owned(),
                memory.to_string(),
                "--cpus".to_owned(),
                format!("{}.{:03}", millicpus / 1_000, millicpus % 1_000),
            ]);
        }
        for spec in binds {
            args.extend(["-v".to_owned(), spec]);
        }
        args.extend(["-w".to_owned(), directory]);
        let mut env = command.env.clone();
        for (name, value) in BASELINE_ENV {
            let value = env.remove(name).unwrap_or_else(|| value.to_owned());
            args.extend(["-e".to_owned(), format!("{name}={value}")]);
        }
        for (name, value) in env {
            args.extend(["-e".to_owned(), format!("{name}={value}")]);
        }
        args.extend([
            "--entrypoint".to_owned(),
            String::new(),
            "--".to_owned(),
            image.to_owned(),
        ]);
        args.extend(command.argv.iter().cloned());
        Ok((args, bounds))
    }
}

/// A writable-roots-only start input, for the wire's lexical rules on writable roots.
fn writable_input(profile: &ConfinementProfile) -> ExecStartInput {
    ExecStartInput {
        workspace: String::new(),
        argv: Vec::new(),
        env: ExecEnvironment {
            allow: Vec::new(),
            set: BTreeMap::new(),
        },
        sandbox: ConfinementRequest {
            capability_snapshot: String::new(),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
            profile: SandboxProfile::Workspace,
            required: true,
        },
        limits: ExecLimits {
            timeout_ms: 0,
            output_bytes: 0,
            processes: 0,
            memory_bytes: 0,
            cpu_millis: 0,
        },
        wait: false,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: std::collections::BTreeSet::new(),
        read_only_roots: profile.read_only_roots.clone(),
        read_only_files: profile.read_only_files.clone(),
        secret_slots: Vec::new(),
        capsule: None,
        lease_ttl_ms: None,
        workdir: None,
        writable_roots: profile
            .writable_roots
            .iter()
            .cloned()
            .map(WritableRoot::Host)
            .collect(),
    }
}

fn check_root(root: &ReadOnlyRoot) -> Result<(), ConfinementError> {
    let path = Path::new(&root.host_path);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Err(refused(
            substrate_wire::READ_ONLY_ROOT_ABSENT,
            substrate_wire::READ_ONLY_ROOT_ABSENT_MESSAGE,
            "read_only_roots",
        ));
    };
    if !metadata.is_dir() || std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(refused(
            substrate_wire::READ_ONLY_ROOT_NOT_A_DIRECTORY,
            substrate_wire::READ_ONLY_ROOT_NOT_A_DIRECTORY_MESSAGE,
            "read_only_roots",
        ));
    }
    Ok(())
}

fn check_file(file: &ReadOnlyFile) -> Result<(), ConfinementError> {
    let path = Path::new(&file.host_path);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Err(refused(
            substrate_wire::READ_ONLY_FILE_ABSENT,
            "A declared read-only file is not present on this host.",
            "read_only_files",
        ));
    };
    if std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(refused(
            substrate_wire::READ_ONLY_FILE_SYMLINK,
            "A declared read-only file is reached through a symlink.",
            "read_only_files",
        ));
    }
    if !metadata.is_file() {
        return Err(refused(
            substrate_wire::READ_ONLY_FILE_NOT_A_FILE,
            "A declared read-only file is not a regular file.",
            "read_only_files",
        ));
    }
    Ok(())
}

/// A variable the runtime's `-e NAME=VALUE` carries exactly: a name with no `=` and no NUL, and a
/// value with no NUL. `-e NAME` without `=` would copy the runtime client's own variable, so every
/// value is always written with its `=`.
fn check_environment(command: &Command) -> Result<(), ConfinementError> {
    let valid = command.env.iter().all(|(name, value)| {
        !name.is_empty() && !name.contains(['=', '\0']) && !value.contains('\0')
    });
    if valid {
        Ok(())
    } else {
        Err(refused(
            "exec.environment-refused",
            "A variable name is empty or holds '=' or NUL, or a value holds NUL.",
            "env",
        ))
    }
}

/// One runtime call, bounded; `true` when it succeeded, with its stdout.
async fn call(program: &Path, args: &[&str]) -> (bool, Vec<u8>) {
    let mut runtime = tokio::process::Command::new(program);
    runtime
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(RUNTIME_CALL_TIMEOUT, runtime.output()).await {
        Ok(Ok(output)) => (output.status.success(), output.stdout),
        _ => (false, Vec::new()),
    }
}

async fn kill(program: &Path, run: &RunId, signal: i32) -> bool {
    call(
        program,
        &["kill", "--signal", &signal.to_string(), run.as_str()],
    )
    .await
    .0
}

/// Removes the run's container if it is still there, then reports whether none with its label
/// remains.
async fn remove(program: &Path, run: &RunId) -> bool {
    let _ = call(program, &["rm", "--force", run.as_str()]).await;
    let filter = format!("label={RUN_LABEL}={}", run.as_str());
    let (listed, remaining) = call(program, &["ps", "--all", "--quiet", "--filter", &filter]).await;
    listed && remaining.iter().all(u8::is_ascii_whitespace)
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

#[async_trait]
impl ConfinementAdapter for OciAdapter {
    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    /// Refuses, after the contract's order, what the runtime could not apply as asked.
    fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
        if !profile.secret_slots.is_empty() {
            return Err(DriverError::unserved(
                "exec.secret-slots-unserved",
                "Sealed secret slots are not served by the oci adapter.",
                "secret_slots",
            )
            .into());
        }
        if !profile.image.as_deref().is_some_and(reference_is_plain) {
            return Err(ConfinementError::ImageUnpinned {
                adapter: AdapterName::Oci,
            });
        }
        if let Some(workdir) = &profile.workdir {
            super::relative_workdir(workdir)?;
        }
        self.bounds(profile)?;
        self.check_mounts(profile)
    }

    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        admitted.check_on(&self.facts, false)?;
        let run = mint();
        let (args, _) = self.prepare(admitted, command, &run)?;
        Ok(RenderedCommand {
            program: self.runtime.program.clone(),
            args,
        })
    }

    async fn start(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        admitted.check_on(&self.facts, true)?;
        let id = mint();
        let (args, bounds) = self.prepare(admitted, command, &id)?;
        self.reserve()?;
        let mut child = tokio::process::Command::new(&self.runtime.program);
        child
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Out of the embedder's process group: a terminal's signal does not reach the client,
            // and the adapter signals the container, never the client.
            .process_group(0);
        let mut child = child.spawn().map_err(|error| {
            DriverError::failed("exec.spawn-failed", format!("oci spawn: {error}"))
        })?;
        let limit = usize::try_from(bounds.output_bytes).unwrap_or(usize::MAX);
        let captured = Arc::new(Mutex::new(Captured::default()));
        let readers = [
            child
                .stdout
                .take()
                .map(|stream| tokio::spawn(drain(stream, Arc::clone(&captured), true, limit))),
            child
                .stderr
                .take()
                .map(|stream| tokio::spawn(drain(stream, Arc::clone(&captured), false, limit))),
        ];
        let signalled = Arc::new(Mutex::new(None));
        let (phase, observed) = watch::channel(Phase::Running);
        let program = self.runtime.program.clone();
        let run = id.clone();
        let sent = Arc::clone(&signalled);
        tokio::spawn(async move {
            let deadline = tokio::time::sleep(bounds.timeout);
            tokio::pin!(deadline);
            let mut ended = tokio::select! {
                status = child.wait() => Some(status),
                () = &mut deadline => None,
            };
            if ended.is_none() {
                *sent.lock() = Some(Signal::Kill.number());
                let grace = tokio::time::sleep(KILL_GRACE);
                tokio::pin!(grace);
                ended = loop {
                    if kill(&program, &run, Signal::Kill.number()).await {
                        break None;
                    }
                    tokio::select! {
                        status = child.wait() => break Some(status),
                        () = tokio::time::sleep(RETRY_PAUSE) => {}
                        () = &mut grace => break None,
                    }
                };
                if ended.is_none() {
                    ended = tokio::time::timeout(KILL_GRACE, child.wait()).await.ok();
                }
                if ended.is_none() {
                    let _ = child.start_kill();
                    ended = Some(child.wait().await);
                }
            }
            for reader in readers.into_iter().flatten() {
                let _ = reader.await;
            }
            let removed = remove(&program, &run).await;
            let signal = *sent.lock();
            let _ = phase.send(match ended {
                Some(Ok(status)) if removed => match status.code() {
                    Some(code) => Phase::Ended(match signal {
                        Some(signal) if code == 128 + signal => RunExit {
                            code: None,
                            signal: Some(signal),
                        },
                        _ => RunExit {
                            code: Some(code),
                            signal: None,
                        },
                    }),
                    None => Phase::Unknown,
                },
                _ => Phase::Unknown,
            });
        });
        self.runs.lock().push(Arc::new(OciRun {
            id: id.clone(),
            phase: observed,
            captured,
            signalled,
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

    /// Returns once the run has ended and its container is gone.
    async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
        let run = self.run(run)?;
        let mut phase = run.phase.clone();
        let _ = phase
            .wait_for(|phase| !matches!(phase, Phase::Running))
            .await;
        Ok(Self::observation(&run))
    }

    /// Signals the container, retrying while the runtime has not created it yet.
    async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        let deadline = Instant::now() + SIGNAL_RETRY;
        loop {
            if !matches!(*run.phase.borrow(), Phase::Running) {
                return Ok(());
            }
            *run.signalled.lock() = Some(signal.number());
            if kill(&self.runtime.program, &run.id, signal.number()).await {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(DriverError::failed(
                    "exec.signal-failed",
                    "oci signal: the container runtime did not deliver the signal",
                )
                .into());
            }
            tokio::time::sleep(RETRY_PAUSE).await;
        }
    }
}

fn mint() -> RunId {
    RunId::new(format!(
        "substrate-oci-{}",
        ulid::Ulid::generate().to_string().to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docker_info(security: &[&str], limits: bool) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "ServerVersion": "29.0.0",
            "SecurityOptions": security,
            "MemoryLimit": limits,
            "SwapLimit": limits,
            "PidsLimit": limits,
            "CpuCfsQuota": limits,
            "NCPU": 4,
        }))
        .expect("json")
    }

    #[test]
    fn docker_info_is_read_for_seccomp_limits_and_rootless() {
        let probe = parse_info(
            PathBuf::from("/usr/bin/docker"),
            &docker_info(&["name=seccomp,profile=builtin", "name=rootless"], true),
        )
        .expect("opens");
        assert_eq!(probe.flavor, RuntimeFlavor::Docker);
        assert!(probe.rootless && probe.limits);
        assert_eq!(probe.cpus, 4);
        let unconfined = parse_info(
            PathBuf::from("/usr/bin/docker"),
            &docker_info(&["name=seccomp,profile=unconfined"], true),
        );
        assert!(unconfined.is_err(), "{unconfined:?}");
        assert!(parse_info(PathBuf::from("/x"), &docker_info(&[], true)).is_err());
        let unbounded = parse_info(
            PathBuf::from("/x"),
            &docker_info(&["name=seccomp,profile=builtin"], false),
        )
        .expect("opens");
        assert!(!unbounded.limits && !unbounded.rootless);
    }

    #[test]
    fn podman_info_is_read_for_seccomp_limits_and_rootless() {
        let info = serde_json::to_vec(&serde_json::json!({
            "host": {
                "security": {"rootless": true, "seccompEnabled": true},
                "cgroupControllers": ["cpu", "memory", "pids"],
                "cpus": 8,
            },
            "version": {"Version": "5.0.0"},
        }))
        .expect("json");
        let probe = parse_info(PathBuf::from("/usr/bin/podman"), &info).expect("opens");
        assert_eq!(probe.flavor, RuntimeFlavor::Podman);
        assert!(probe.rootless && probe.limits);
        assert_eq!(probe.version, "5.0.0");
    }

    #[test]
    fn only_a_plain_reference_reaches_the_command_line() {
        let digest = "0".repeat(64);
        for plain in ["registry.invalid/run", "localhost:5000/a_b-c.d", "alpine"] {
            assert!(
                reference_is_plain(&format!("{plain}@sha256:{digest}")),
                "{plain}"
            );
        }
        for odd in ["-v/:/host", "--privileged", "Upper/case", "a b", ""] {
            assert!(
                !reference_is_plain(&format!("{odd}@sha256:{digest}")),
                "{odd}"
            );
        }
    }
}
