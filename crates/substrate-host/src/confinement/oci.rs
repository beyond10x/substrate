//! The `oci` adapter: one container per run under Docker or podman, the image by digest only
//! (atlas ADR 0109 decisions 3 and 5, ADR 0044 decision 8).
//!
//! It is the `sandbox` repository's `docker run` posture behind the confinement contract, plus
//! what `sandbox` lacks. Every run is created with
//!
//! | flag | why |
//! |---|---|
//! | `create --name <run> --label …` | one container per run, findable by label |
//! | `--label …embedder=<identity>` | the embedding process (boot, pid namespace, pid, start time): `open` removes a container whose embedder is gone |
//! | `--pull never` | no registry is contacted inside a run; an absent image is `confinement.image-absent` |
//! | `--log-driver none` | the runtime keeps no unbounded copy of the output; the adapter drains and bounds it |
//! | `--init` | the command is not PID 1, so a signal the adapter sends ends it as it does under `local` |
//! | `--user <uid>:<gid>` | the embedding process's own identity, so the binds grant nothing it lacks; never 0 |
//! | `--network none`, `--ipc none` | no network and no shared IPC: no `/dev/shm`, `network_egress` and `network_ingress` not served |
//! | `--cap-drop ALL`, `--security-opt no-new-privileges` | no capability, no gain through exec |
//! | `--read-only --tmpfs /tmp` | the image's root is read-only; `/tmp` is private |
//! | `--pids-limit`, `--memory`, `--memory-swap`, `--cpus` | the profile's bounds or [`DEFAULT_BOUNDS`], always |
//! | `--mount type=bind,source=<workspace>,target=/workspace` | the one writable workspace, directly beneath the operator's workspace root |
//! | `--mount type=bind,…,readonly` | read-only roots and files |
//! | `--mount type=bind,…` | operator-declared writable roots |
//! | `-w`, five baseline `-e` variables, then the command's | the start directory and the whole environment, shaped by the `linux` driver's rule |
//! | `--entrypoint "" -- <name>@sha256:<64 hex>` | the image by digest, the command as given |
//!
//! then started with `start --attach`, read with `container inspect` once it ends (its exit code,
//! and whether the kernel's OOM killer ended it), and removed. A run is not started until the
//! runtime reports its container started; a container the runtime did not create or start is a
//! named refusal, never a run that ended.
//!
//! The runtime's default seccomp profile is kept: no flag here touches it, and `open` refuses a
//! runtime whose `info` does not report seccomp. `open` probes the runtime (the program, `info`
//! answering, seccomp, the cgroup controllers behind `limits`, rootless or not, the sockets it is
//! reached through) and fails closed as `confinement.adapter-unserved`, as it does for an
//! embedder running as uid 0. `limits` is served only when the runtime reports memory, swap, pids
//! and CPU quota control; without it no run starts (`exec.limit-unserved`), so no bound is ever
//! silently discarded (invariant 3).
//!
//! It serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
//! `filesystem_view`, `limits` and `audit`, and proves `image_digest_pinned`. It does not prove
//! `ipc_socket_allowlist`: a Unix socket beneath a bound root is reachable from the container.
//! Whether the runtime is rootless has no `Fact` variant; [`probe`] reports it.
//!
//! The runtime's socket is root-equivalent on a rootful host
//! (`docs/design/04-security-and-isolation.md`). The adapter's authority is the operator's runtime;
//! a profile reaches it only through the fixed flags above, every value a separate argument, and
//! the image after `--`. No bind is, or holds, a socket the runtime is reached through.

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
/// The label naming the process that created a container: `<boot id>/<pid namespace>/<pid>/<start
/// time in clock ticks>`.
pub const EMBEDDER_LABEL: &str = "dev.b10x.substrate.confinement.embedder";
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
/// A host path or mount the runtime's `--mount` value cannot carry exactly: it holds a `,` (the
/// value's field separator), a `"`, a line break, NUL, or a `:`.
pub const PATH_UNREPRESENTABLE: &str = "confinement.path-unrepresentable";
/// A bound host path that is, or holds, a socket the container runtime is reached through.
pub const RUNTIME_SOCKET_EXPOSED: &str = "confinement.runtime-socket-exposed";
/// A pinned image the runtime does not hold: nothing is pulled inside a run.
pub const IMAGE_ABSENT: &str = "confinement.image-absent";

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const RUNTIME_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const SIGNAL_RETRY: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_millis(100);
const STARTED_POLL: Duration = Duration::from_millis(50);
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
/// Where Docker and podman listen when nothing else is configured.
const WELL_KNOWN_SOCKETS: [&str; 4] = [
    "/run/docker.sock",
    "/var/run/docker.sock",
    "/run/podman/podman.sock",
    "/var/run/podman/podman.sock",
];

/// What opening `oci` takes.
#[derive(Debug, Clone)]
pub struct OciRequest {
    /// The container runtime program: an absolute path, or a bare name (`docker`, `podman`)
    /// looked up on `PATH`.
    pub runtime: PathBuf,
    /// The operator's workspace root: a profile's workspace must be a directory directly beneath
    /// it, as under `linux`; anything else is `workspace.path-escape`.
    pub workspace_root: PathBuf,
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
    /// The Unix sockets the runtime is reached through, as named and as resolved: the one it
    /// reports, the ones its client's environment names, and the well-known paths. No bind is, or
    /// holds, one of them.
    pub sockets: Vec<PathBuf>,
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
    let mut probe = parse_info(program, &info).map_err(unserved)?;
    if probe.flavor == RuntimeFlavor::Docker
        && let Some(host) = run_bounded(
            &probe.program,
            &[
                "context",
                "inspect",
                "--format",
                "{{.Endpoints.docker.Host}}",
            ],
            PROBE_TIMEOUT,
        )
    {
        probe
            .sockets
            .extend(socket_path(String::from_utf8_lossy(&host).trim()));
    }
    for variable in ["DOCKER_HOST", "CONTAINER_HOST"] {
        if let Some(value) = std::env::var_os(variable) {
            probe.sockets.extend(socket_path(&value.to_string_lossy()));
        }
    }
    probe
        .sockets
        .extend(WELL_KNOWN_SOCKETS.iter().map(PathBuf::from));
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime_dir = PathBuf::from(runtime_dir);
        probe.sockets.push(runtime_dir.join("docker.sock"));
        probe.sockets.push(runtime_dir.join("podman/podman.sock"));
    }
    let resolved: Vec<PathBuf> = probe
        .sockets
        .iter()
        .filter_map(|socket| std::fs::canonicalize(socket).ok())
        .collect();
    probe.sockets.extend(resolved);
    probe.sockets.retain(|socket| socket.is_absolute());
    probe.sockets.sort();
    probe.sockets.dedup();
    Ok(probe)
}

/// The path of a `unix://` endpoint, or of a bare absolute path.
fn socket_path(endpoint: &str) -> Option<PathBuf> {
    let path = endpoint.strip_prefix("unix://").unwrap_or(endpoint);
    Path::new(path).is_absolute().then(|| PathBuf::from(path))
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
    let (flavor, version, seccomp, rootless, limits, cpus, sockets) =
        if let Some(host) = info.get("host") {
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
                host.pointer("/remoteSocket/path")
                    .and_then(Value::as_str)
                    .and_then(socket_path)
                    .into_iter()
                    .collect(),
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
                Vec::new(),
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
        sockets,
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
            && root
                .path
                .to_str()
                .is_some_and(|path| !unrepresentable(path));
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

/// The operator's workspace root, resolved: an absolute directory the runtime can bind beneath.
fn check_workspace_root(root: &Path) -> Result<PathBuf, ConfinementError> {
    std::fs::canonicalize(root)
        .ok()
        .filter(|resolved| {
            root.is_absolute()
                && resolved.is_dir()
                && resolved.to_str().is_some_and(|path| !unrepresentable(path))
        })
        .ok_or_else(|| {
            DriverError::refused(
                "config.workspace-root-invalid",
                "The oci workspace root is not an absolute directory the runtime can bind beneath.",
                "config.workspace_root",
            )
            .into()
        })
}

/// An embedder running as uid 0 would run every container as root (`--user 0:0`).
fn check_embedder_uid(uid: u32) -> Result<(), ConfinementError> {
    if uid == 0 {
        return Err(unserved(
            "an embedder running as uid 0 would run every container as root",
        ));
    }
    Ok(())
}

/// The process that creates containers, as its label names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Embedder {
    boot: String,
    pid_namespace: String,
    pid: u32,
    started: u64,
}

impl Embedder {
    /// This process, when the host reports every part of its identity.
    fn current() -> Option<Self> {
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        let pid = std::process::id();
        Some(Self {
            boot: boot.trim().to_owned(),
            pid_namespace: pid_namespace(Path::new("/proc/self/ns/pid"))?,
            pid,
            started: process_start(pid)?,
        })
        .filter(|embedder| Self::parse(&embedder.label()).as_ref() == Some(embedder))
    }

    fn label(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.boot, self.pid_namespace, self.pid, self.started
        )
    }

    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('/');
        let embedder = Self {
            boot: parts.next()?.to_owned(),
            pid_namespace: parts.next()?.to_owned(),
            pid: parts.next()?.parse().ok()?,
            started: parts.next()?.parse().ok()?,
        };
        let plain = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        };
        (parts.next().is_none() && plain(&embedder.boot) && plain(&embedder.pid_namespace))
            .then_some(embedder)
    }

    /// Whether `owner` is a process this one can judge: same boot, same pid namespace.
    fn judges(&self, owner: &Self) -> bool {
        self.boot == owner.boot && self.pid_namespace == owner.pid_namespace
    }

    fn alive(&self) -> bool {
        process_start(self.pid) == Some(self.started)
    }
}

/// The inode of a pid namespace link, `pid:[<inode>]`.
fn pid_namespace(link: &Path) -> Option<String> {
    let target = std::fs::read_link(link).ok()?;
    let inode = target
        .to_str()?
        .strip_prefix("pid:[")?
        .strip_suffix(']')?
        .to_owned();
    inode
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then_some(inode)
}

/// A process's start time in clock ticks since boot (`/proc/<pid>/stat` field 22).
fn process_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Removes every `oci` container whose embedder this process can judge and is gone, and returns
/// how many it removed. A runtime that does not list or inspect leaves them, and nothing fails.
fn sweep(program: &Path, embedder: &Embedder) -> usize {
    let filter = format!("label={LABEL}=oci");
    let Some(listed) = run_bounded(
        program,
        &["ps", "--all", "--quiet", "--no-trunc", "--filter", &filter],
        PROBE_TIMEOUT,
    ) else {
        return 0;
    };
    let format = format!("{{{{index .Config.Labels \"{EMBEDDER_LABEL}\"}}}}");
    String::from_utf8_lossy(&listed)
        .split_whitespace()
        .filter(|id| id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .filter(|id| {
            run_bounded(
                program,
                &["container", "inspect", "--format", &format, id],
                PROBE_TIMEOUT,
            )
            .and_then(|owner| Embedder::parse(String::from_utf8_lossy(&owner).trim()))
            .is_some_and(|owner| embedder.judges(&owner) && !owner.alive())
        })
        .filter(|id| run_bounded(program, &["rm", "--force", id], PROBE_TIMEOUT).is_some())
        .count()
}

/// Opens `oci` and returns the adapter itself, whose [`OciAdapter::orphans_removed`] reports what
/// opening removed. [`super::open`] answers the same refusals.
///
/// # Errors
///
/// `confinement.adapter-unserved` when the runtime does not open or the embedder runs as uid 0;
/// `config.writable-root-invalid` or `config.workspace-root-invalid` for the request itself.
pub fn open_adapter(request: OciRequest) -> Result<Arc<OciAdapter>, ConfinementError> {
    check_embedder_uid(nix::unistd::getuid().as_raw())?;
    check_declared(&request.writable_roots)?;
    let runtime = probe(&request)?;
    let workspace_root = check_workspace_root(&request.workspace_root)?;
    let embedder = Embedder::current();
    let orphans_removed = embedder
        .as_ref()
        .map_or(0, |embedder| sweep(&runtime.program, embedder));
    if orphans_removed > 0 {
        tracing::warn!(
            orphans_removed,
            "oci removed containers whose embedder is gone"
        );
    }
    let adapter = OciAdapter {
        facts: proof(&runtime),
        runtime,
        workspace_root,
        writable_roots: request.writable_roots,
        identity: (
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw(),
        ),
        embedder,
        orphans_removed,
        runs: Mutex::new(Vec::new()),
    };
    super::check_open(&adapter.facts)?;
    Ok(Arc::new(adapter))
}

pub(crate) fn open(request: OciRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let adapter: Arc<dyn ConfinementAdapter> = open_adapter(request)?;
    Ok(adapter)
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

fn unrepresentable(path: &str) -> bool {
    path.contains([':', ',', '"', '\n', '\r', '\0'])
}

fn representable(path: &str, field: &str) -> Result<(), ConfinementError> {
    if unrepresentable(path) {
        return Err(DriverError::refused(
            PATH_UNREPRESENTABLE,
            "A path holds a ',', '\"', ':', line break or NUL, which the container runtime's \
             bind syntax cannot carry exactly.",
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
    /// `--pids-limit`, `--memory` and `--memory-swap`, and `--cpus` in thousandths.
    limits: (u32, u64, u64),
}

/// One bind, as the runtime's `--mount` value. A missing source is refused by the runtime, never
/// created.
fn bind(host: &str, mount: &str, read_only: bool) -> String {
    let spec = format!("type=bind,source={host},target={mount}");
    if read_only {
        format!("{spec},readonly")
    } else {
        spec
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

fn create_failed(message: &str) -> ConfinementError {
    DriverError::failed("exec.spawn-failed", message).into()
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
}

/// A container's state as the runtime's `container inspect` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContainerState {
    started: bool,
    running: bool,
    oom_killed: bool,
    exit_code: Option<i64>,
}

impl ContainerState {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let state: Value = serde_json::from_slice(bytes).ok()?;
        Some(Self {
            started: state
                .get("StartedAt")
                .and_then(Value::as_str)
                .is_some_and(|at| !at.is_empty() && !at.starts_with("0001-")),
            running: flag(&state, "Running"),
            oom_killed: flag(&state, "OOMKilled"),
            exit_code: state.get("ExitCode").and_then(Value::as_i64),
        })
    }

    /// How the command ended: `128 + n` is signal `n`, whoever sent it, and a kill by the OOM
    /// killer is signal 9, as `local` and `linux` report them.
    fn exit(&self) -> Option<RunExit> {
        let code = i32::try_from(self.exit_code?).ok()?;
        let signal = |signal| RunExit {
            code: None,
            signal: Some(signal),
        };
        Some(match code {
            129..=192 => signal(code - 128),
            _ if self.oom_killed && !(0..=128).contains(&code) => signal(Signal::Kill.number()),
            _ => RunExit {
                code: Some(code),
                signal: None,
            },
        })
    }
}

pub struct OciAdapter {
    facts: AdapterFacts,
    runtime: RuntimeProbe,
    workspace_root: PathBuf,
    writable_roots: Vec<DeclaredWritableRoot>,
    identity: (u32, u32),
    embedder: Option<Embedder>,
    orphans_removed: usize,
    runs: Mutex<Vec<Arc<OciRun>>>,
}

impl OciAdapter {
    /// The containers `open` removed because the process that created them is gone.
    pub fn orphans_removed(&self) -> usize {
        self.orphans_removed
    }

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
            memory_oom_kills: None,
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

    /// The profile's bounds, or the defaults, checked against what a runtime accepts. A runtime
    /// that cannot apply them runs nothing.
    fn bounds(&self, profile: &ConfinementProfile) -> Result<RunBounds, ConfinementError> {
        if !self.facts.serves(&ContractFamily::Limits) {
            return Err(limit_unserved(
                "The container runtime reports no pids, memory, swap and CPU control, and an oci \
                 run never starts without its bounds.",
            ));
        }
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
            limits: (bounds.processes, bounds.memory_bytes, millicpus),
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

    /// Refuses a host path that is, or holds, a socket the runtime is reached through.
    fn check_socket(&self, host: &Path, field: &str) -> Result<(), ConfinementError> {
        if self
            .runtime
            .sockets
            .iter()
            .any(|socket| socket.starts_with(host))
        {
            return Err(refused(
                RUNTIME_SOCKET_EXPOSED,
                "A bound host path is, or holds, the container runtime's socket.",
                field,
            ));
        }
        Ok(())
    }

    /// The `--mount` values and the start directory, each host path checked now.
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
        if workspace.parent() != Some(self.workspace_root.as_path()) {
            return Err(refused(
                "workspace.path-escape",
                "The oci adapter's workspace must be a directory directly beneath its workspace \
                 root.",
                "workspace",
            ));
        }
        self.check_socket(workspace, "workspace")?;
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
            self.check_socket(Path::new(&root.host_path), "read_only_roots")?;
            binds.push(bind(&root.host_path, &root.mount, true));
        }
        for file in &profile.read_only_files {
            check_file(file)?;
            self.check_socket(Path::new(&file.host_path), "read_only_files")?;
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
            self.check_socket(&declared.path, "writable_roots")?;
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

    /// Everything one start or render needs, checked: the runtime's `create` arguments, the
    /// image and the bounds.
    fn prepare<'profile>(
        &self,
        admitted: &'profile AdmittedProfile,
        command: &Command,
        run: &RunId,
    ) -> Result<(Vec<String>, &'profile str, RunBounds), ConfinementError> {
        super::check_command(command)?;
        let profile = admitted.profile();
        // The environment rule every adapter holds a run to (`crate::environment`).
        crate::environment::check(&command.env)?;
        let image = profile
            .image
            .as_deref()
            .filter(|image| super::image_is_pinned(image) && reference_is_plain(image))
            .ok_or(ConfinementError::ImageUnpinned {
                adapter: AdapterName::Oci,
            })?;
        let bounds = self.bounds(profile)?;
        let (binds, directory) = self.mounts(admitted)?;
        let mut args: Vec<String> = vec![
            "create".to_owned(),
            "--name".to_owned(),
            run.as_str().to_owned(),
            "--label".to_owned(),
            format!("{LABEL}=oci"),
            "--label".to_owned(),
            format!("{RUN_LABEL}={}", run.as_str()),
        ];
        if let Some(embedder) = &self.embedder {
            args.extend([
                "--label".to_owned(),
                format!("{EMBEDDER_LABEL}={}", embedder.label()),
            ]);
        }
        args.extend(
            ["--pull", "never", "--log-driver", "none", "--init"]
                .iter()
                .map(|arg| (*arg).to_owned()),
        );
        if self.runtime.flavor == RuntimeFlavor::Podman && self.runtime.rootless {
            args.push("--userns=keep-id".to_owned());
        }
        let (uid, gid) = self.identity;
        let (processes, memory, millicpus) = bounds.limits;
        args.extend([
            "--user".to_owned(),
            format!("{uid}:{gid}"),
            "--network".to_owned(),
            "none".to_owned(),
            "--ipc".to_owned(),
            "none".to_owned(),
            "--cap-drop".to_owned(),
            "ALL".to_owned(),
            "--security-opt".to_owned(),
            "no-new-privileges".to_owned(),
            "--read-only".to_owned(),
            "--tmpfs".to_owned(),
            "/tmp".to_owned(),
            "--pids-limit".to_owned(),
            processes.to_string(),
            "--memory".to_owned(),
            memory.to_string(),
            "--memory-swap".to_owned(),
            memory.to_string(),
            "--cpus".to_owned(),
            format!("{}.{:03}", millicpus / 1_000, millicpus % 1_000),
        ]);
        for spec in binds {
            args.extend(["--mount".to_owned(), spec]);
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
        Ok((args, image, bounds))
    }

    fn render_checked(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        admitted.check_on(&self.facts, false)?;
        let run = mint();
        let (args, _, _) = self.prepare(admitted, command, &run)?;
        Ok(RenderedCommand {
            program: self.runtime.program.clone(),
            args,
        })
    }

    /// Creates the run's container and starts it attached, returning once the runtime reports it
    /// started. Nothing the runtime prints about its own failure reaches the run's output.
    async fn start_checked(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        admitted.check_on(&self.facts, true)?;
        let id = mint();
        let (args, image, bounds) = self.prepare(admitted, command, &id)?;
        self.reserve()?;
        let program = self.runtime.program.clone();
        create(&program, &args, image, &id).await?;
        let mut client = tokio::process::Command::new(&program);
        client
            .args(["start", "--attach", id.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Out of the embedder's process group: a terminal's signal does not reach the client,
            // and the adapter signals the container, never the client.
            .process_group(0);
        let Ok(mut child) = client.spawn() else {
            let _ = remove(&program, &id).await;
            return Err(create_failed("The container runtime client did not run."));
        };
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
        if !started(&program, &id, &mut child).await {
            let _ = child.start_kill();
            let _ = child.wait().await;
            for reader in readers.into_iter().flatten() {
                reader.abort();
            }
            let _ = remove(&program, &id).await;
            return Err(create_failed(
                "The container runtime did not start the run's container.",
            ));
        }
        let (phase, observed) = watch::channel(Phase::Running);
        tokio::spawn(monitor(
            program,
            id.clone(),
            child,
            readers,
            bounds.timeout,
            phase,
        ));
        self.runs.lock().push(Arc::new(OciRun {
            id: id.clone(),
            phase: observed,
            captured,
        }));
        admitted.audit_start(&id, &command.argv[0]);
        Ok(id)
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

async fn inspect(program: &Path, run: &RunId) -> Option<ContainerState> {
    let (ok, out) = call(
        program,
        &[
            "container",
            "inspect",
            "--format",
            "{{json .State}}",
            run.as_str(),
        ],
    )
    .await;
    ok.then(|| ContainerState::parse(&out)).flatten()
}

/// Creates the run's container from its `create` arguments. A container the runtime did not
/// create is removed if any of it exists, and refused by name: `confinement.image-absent` when the
/// runtime does not hold the image, `exec.spawn-failed` otherwise. The runtime's own error text
/// is not passed on.
async fn create(
    program: &Path,
    args: &[String],
    image: &str,
    run: &RunId,
) -> Result<(), ConfinementError> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if call(program, &args).await.0 {
        return Ok(());
    }
    let _ = remove(program, run).await;
    let present = call(program, &["image", "inspect", "--format", "{{.Id}}", image])
        .await
        .0;
    Err(if present {
        create_failed("The container runtime did not create the run's container.")
    } else {
        refused(
            IMAGE_ABSENT,
            "The container runtime does not hold the pinned image, and a run pulls nothing.",
            "image",
        )
    })
}

/// Watches one started run to its end: its timeout kills the container, then the output is
/// drained, the container's state read and the container removed before the phase is published.
async fn monitor(
    program: PathBuf,
    run: RunId,
    mut child: tokio::process::Child,
    readers: [Option<tokio::task::JoinHandle<()>>; 2],
    timeout: Duration,
    phase: watch::Sender<Phase>,
) {
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut ended = tokio::select! {
        status = child.wait() => Some(status),
        () = &mut deadline => None,
    };
    if ended.is_none() {
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
    let state = inspect(&program, &run).await;
    let removed = remove(&program, &run).await;
    let _ = phase.send(match (ended, state) {
        (Some(Ok(_)), Some(state)) if removed && !state.running => {
            state.exit().map_or(Phase::Unknown, Phase::Ended)
        }
        _ => Phase::Unknown,
    });
}

/// Waits until the runtime reports the container started; `false` when the client ended first
/// with the container never started, or the runtime did not report it within its bound.
async fn started(program: &Path, run: &RunId, client: &mut tokio::process::Child) -> bool {
    let deadline = Instant::now() + RUNTIME_CALL_TIMEOUT;
    loop {
        if inspect(program, run)
            .await
            .is_some_and(|state| state.started)
        {
            return true;
        }
        if !matches!(client.try_wait(), Ok(None)) {
            return inspect(program, run)
                .await
                .is_some_and(|state| state.started);
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(STARTED_POLL).await;
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

    /// The runtime's `create` call for `command`; a start follows it with `start --attach`. A
    /// refused render is audited as a refused start is.
    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        admitted.audit_refusal(&self.facts, self.render_checked(admitted, command))
    }

    async fn start(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        let started = self.start_checked(admitted, command).await;
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

    /// Returns once the run has ended and its container is gone.
    async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
        let run = self.run(run)?;
        let mut phase = run.phase.clone();
        let _ = phase
            .wait_for(|phase| !matches!(phase, Phase::Running))
            .await;
        Ok(Self::observation(&run))
    }

    /// Signals the container's init, which passes the signal to the command.
    async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        let deadline = Instant::now() + SIGNAL_RETRY;
        loop {
            if !matches!(*run.phase.borrow(), Phase::Running) {
                return Ok(());
            }
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
    fn podman_info_is_read_for_seccomp_limits_rootless_and_its_socket() {
        let info = serde_json::to_vec(&serde_json::json!({
            "host": {
                "security": {"rootless": true, "seccompEnabled": true},
                "cgroupControllers": ["cpu", "memory", "pids"],
                "cpus": 8,
                "remoteSocket": {"exists": true, "path": "unix:///run/user/1000/podman/podman.sock"},
            },
            "version": {"Version": "5.0.0"},
        }))
        .expect("json");
        let probe = parse_info(PathBuf::from("/usr/bin/podman"), &info).expect("opens");
        assert_eq!(probe.flavor, RuntimeFlavor::Podman);
        assert!(probe.rootless && probe.limits);
        assert_eq!(probe.version, "5.0.0");
        assert_eq!(
            probe.sockets,
            [PathBuf::from("/run/user/1000/podman/podman.sock")]
        );
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

    /// Security F5 (`story:oci-adapter`): a root embedder would run every container as root.
    #[test]
    fn a_root_embedder_is_unserved() {
        let error = check_embedder_uid(0).expect_err("uid 0 is refused");
        assert_eq!(error.code(), "confinement.adapter-unserved");
        assert!(check_embedder_uid(1000).is_ok());
    }

    /// Security F7 (`story:oci-adapter`): every character the `--mount` value cannot carry is
    /// refused, and a read-only bind says so.
    #[test]
    fn a_mount_value_carries_its_paths_exactly() {
        for odd in ["/a,b", "/a:b", "/a\"b", "/a\nb", "/a\rb"] {
            assert_eq!(
                representable(odd, "f").expect_err(odd).code(),
                PATH_UNREPRESENTABLE
            );
        }
        assert_eq!(
            bind("/srv/a=b", "/opt/x", true),
            "type=bind,source=/srv/a=b,target=/opt/x,readonly"
        );
        assert_eq!(
            bind("/srv/w", "/workspace", false),
            "type=bind,source=/srv/w,target=/workspace"
        );
    }

    /// Security F6 (`story:oci-adapter`): an embedder label reads back exactly, a live embedder is
    /// alive, the same pid with another start time is gone, and another boot or pid namespace is
    /// never judged. Linux only (`story:oci-adapter`): the identity is read from `/proc`, and
    /// where it is absent `Embedder::current` is `None` and open sweeps nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_embedder_label_names_one_process_and_judges_only_its_own_namespace() {
        let this = Embedder::current().expect("this host reports a process identity");
        assert_eq!(Embedder::parse(&this.label()), Some(this.clone()));
        assert!(this.alive());
        let restarted = Embedder {
            started: this.started + 1,
            ..this.clone()
        };
        assert!(this.judges(&restarted) && !restarted.alive());
        let elsewhere = Embedder {
            pid_namespace: "1".into(),
            ..this.clone()
        };
        assert!(!this.judges(&elsewhere));
        for odd in ["", "a/b/c", "a/1/2/3/4", "a b/1/2/3", "a/1/x/3", "a/1/2/-3"] {
            assert_eq!(Embedder::parse(odd), None, "{odd}");
        }
    }

    /// Adversary 3 (`story:oci-adapter`): a signal the adapter did not send is still a signal, and
    /// an OOM kill is signal 9.
    #[test]
    fn a_container_state_reads_back_as_local_reports_it() {
        let state = |code: i64, oom: bool| ContainerState {
            started: true,
            running: false,
            oom_killed: oom,
            exit_code: Some(code),
        };
        let signal = |signal| {
            Some(RunExit {
                code: None,
                signal: Some(signal),
            })
        };
        assert_eq!(state(137, true).exit(), signal(9));
        assert_eq!(state(137, false).exit(), signal(9));
        assert_eq!(state(143, false).exit(), signal(15));
        assert_eq!(state(255, true).exit(), signal(9));
        assert_eq!(
            state(3, true).exit(),
            Some(RunExit {
                code: Some(3),
                signal: None
            })
        );
        let parsed = ContainerState::parse(
            br#"{"Status":"exited","Running":false,"OOMKilled":true,"ExitCode":137,
                "StartedAt":"2026-10-27T10:00:00.1Z"}"#,
        )
        .expect("parsed");
        assert_eq!(parsed, state(137, true));
        let never = ContainerState::parse(
            br#"{"Status":"created","Running":false,"OOMKilled":false,"ExitCode":0,
                "StartedAt":"0001-01-01T00:00:00Z"}"#,
        )
        .expect("parsed");
        assert!(!never.started);
    }
}
