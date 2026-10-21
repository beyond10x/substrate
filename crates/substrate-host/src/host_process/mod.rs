//! Host processes (ADR 0036, design 26): the driver half.
//!
//! A host process runs with the `host` profile (S2): the daemon's own uid, the host mount,
//! network, PID, IPC and user namespaces, no bubblewrap, no seccomp filter, no capsule and no
//! workspace. Every result this module returns says so — `profile: host`, `confinement: none` —
//! and what it does enforce is listed beside that: one cgroup per process under the daemon's
//! delegated root, whole-tree termination, hard deadlines and a bounded log.
//!
//! This is the driver: no HTTP, no store, no lease clock. The daemon records each operation
//! durably before calling in (invariant 5), triggers lease expiry from its own clock and calls
//! [`HostProcessSupervisor::stop`] with [`StopCause::LeaseExpiry`]. Deadlines (S11) are enforced
//! here, because they are fixed at start and need no store to fire.
//!
//! **A missing delegated cgroup is a named refusal.** Without a usable delegated cgroup v2 root
//! [`HostProcessSupervisor::open`] answers `process.host-unserved` and nothing ever runs; there is
//! no path on which a host process starts outside a cgroup of its own (invariant 3).
//!
//! Exec, sessions and their isolation floor are untouched: nothing here is reachable from
//! `ProcessRuntime`, and nothing there from here, except the shared `cgroup.kill`, CPU and
//! empty-cgroup removal primitives this module reuses. Its `SIGTERM` and its removal walk the
//! record's whole cgroup subtree, which exec's primitives do not (`tree`).

mod log;
#[cfg(target_os = "linux")]
mod pidfd;
mod ports;
#[cfg(target_os = "linux")]
#[cfg(test)]
mod security_tests;
#[cfg(target_os = "linux")]
#[cfg(test)]
mod tests;
#[cfg(target_os = "linux")]
mod tree;
#[cfg(not(target_os = "linux"))]
mod unsupported;

use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::fs::OpenOptions;
use std::os::fd::{AsFd as _, AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use parking_lot::Mutex;
use substrate_wire::MIN_LEASE_TTL_MS;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::watch;

#[cfg(target_os = "linux")]
use crate::process::{Cgroup, read_decimal_counter};
use crate::{DriverError, DriverErrorClass};

pub use log::{
    LogLine, LogStream, LogStreamSelection, LogTail, MAX_PROCESS_LOG_BYTES,
    MAX_PROCESS_LOG_LINE_BYTES,
};
pub use ports::{ListeningSocket, SocketProtocol};
#[cfg(not(target_os = "linux"))]
pub use unsupported::HostProcessSupervisor;

use log::ProcessLog;

// The bounds below are the wire's (ADR 0036 § named refusals: every bound is a wire constant of
// the successor bundle); the driver re-exports them so its own checks cannot drift from them.

/// The most `argv` items a host process may be started with.
pub const MAX_PROCESS_ARGV_ITEMS: usize = substrate_wire::MAX_PROCESS_ARGV_ITEMS;
/// The most `argv` bytes, all items together.
pub const MAX_PROCESS_ARGV_BYTES: usize = substrate_wire::MAX_PROCESS_ARGV_BYTES;
/// The most environment entries.
pub const MAX_PROCESS_ENV_ITEMS: usize = substrate_wire::MAX_PROCESS_ENV_ITEMS;
/// The most environment bytes, every name and value together.
pub const MAX_PROCESS_ENV_BYTES: usize = substrate_wire::MAX_PROCESS_ENV_BYTES;
/// The longest `timeout_ms`, and the furthest an `expires_at` may lie ahead (S11).
pub const MAX_PROCESS_TIMEOUT_MS: u64 = substrate_wire::MAX_PROCESS_TIMEOUT_MS;
/// The grace between `SIGTERM` and `cgroup.kill` on every stop (S3).
pub const DEFAULT_PROCESS_STOP_GRACE: Duration =
    Duration::from_millis(substrate_wire::PROCESS_STOP_GRACE_MS);
/// The longest grace a stop may ask for.
pub const MAX_PROCESS_STOP_GRACE: Duration = Duration::from_mins(1);
/// The largest tail one log read returns.
pub const MAX_PROCESS_LOG_TAIL_BYTES: u64 = substrate_wire::MAX_PROCESS_LOG_TAIL_BYTES;

/// Delegated cgroup v2 root absent or unusable for process cgroups (S2).
pub const HOST_UNSERVED: &str = substrate_wire::PROCESS_HOST_UNSERVED;
pub const ARGV_INVALID: &str = substrate_wire::PROCESS_ARGV_INVALID;
pub const CWD_INVALID: &str = substrate_wire::PROCESS_CWD_INVALID;
pub const ENV_INVALID: &str = substrate_wire::PROCESS_ENV_INVALID;
pub const TIMEOUT_INVALID: &str = substrate_wire::PROCESS_TIMEOUT_INVALID;
pub const EXPIRES_AT_INVALID: &str = substrate_wire::PROCESS_EXPIRES_AT_INVALID;
pub const NOT_FOUND: &str = substrate_wire::PROCESS_NOT_FOUND;
/// The program could not be executed: absent, not executable, or refused by the kernel.
pub const SPAWN_FAILED: &str = "process.spawn-failed";
/// Driver-internal: an identity the daemon passed in is malformed or already held.
pub const ID_INVALID: &str = "process.id-invalid";
/// Driver-internal: a stop grace or a tail budget outside its bound.
pub const BOUND_INVALID: &str = "process.bound-invalid";
/// Driver-internal: forgetting a record that has not ended.
pub const NOT_ENDED: &str = "process.not-ended";
/// The cgroup could not be created, emptied or removed.
pub const CGROUP_FAILED: &str = "process.cgroup-failed";

const CGROUP_PREFIX: &str = "substrate-hp_";
const EMPTY_POLL: Duration = Duration::from_millis(10);
/// How long a cgroup may take to empty after `cgroup.kill` before the stop is a failure.
const KILL_SETTLE: Duration = Duration::from_secs(10);
/// How long the output readers may take to drain after the cgroup is empty.
const DRAIN_SETTLE: Duration = Duration::from_secs(2);
const MAX_ID_BYTES: usize = 64;

/// The confinement profile. `host` is the only one this family admits (S2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProfile {
    Host,
}

/// The confinement a host process has: none, reported on every result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    None,
}

/// Which deadline field set the effective deadline (S11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineCause {
    Timeout,
    ExpiresAt,
}

/// The effective hard deadline: one absolute instant and the field that set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    pub at: DateTime<Utc>,
    pub cause: DeadlineCause,
}

impl Deadline {
    /// The effective deadline of a start at `started_at` (S11): the earlier of
    /// `started_at + timeout_ms` and `expires_at`, or none when neither is given. On a tie the
    /// timeout names it.
    ///
    /// # Errors
    ///
    /// `process.timeout-invalid` for a zero timeout or one above [`MAX_PROCESS_TIMEOUT_MS`];
    /// `process.expires-at-invalid` for an `expires_at` that is not an RFC 3339 UTC instant, is
    /// not at least `MIN_LEASE_TTL_MS` after `started_at`, or lies beyond
    /// [`MAX_PROCESS_TIMEOUT_MS`] from it.
    pub fn effective(
        timeout_ms: Option<u64>,
        expires_at: Option<&str>,
        started_at: DateTime<Utc>,
    ) -> Result<Option<Self>, DriverError> {
        let timeout = timeout_ms
            .map(|timeout| {
                if timeout == 0 || timeout > MAX_PROCESS_TIMEOUT_MS {
                    return Err(DriverError::refused(
                        TIMEOUT_INVALID,
                        format!("timeout_ms must lie in 1..={MAX_PROCESS_TIMEOUT_MS}."),
                        "timeout_ms",
                    ));
                }
                Ok(Self {
                    at: started_at + millis(timeout),
                    cause: DeadlineCause::Timeout,
                })
            })
            .transpose()?;
        let expiry = expires_at
            .map(|value| {
                let refused = || {
                    DriverError::refused(
                        EXPIRES_AT_INVALID,
                        format!(
                            "expires_at must be an RFC 3339 UTC instant at least \
                             {MIN_LEASE_TTL_MS} ms and at most {MAX_PROCESS_TIMEOUT_MS} ms ahead."
                        ),
                        "expires_at",
                    )
                };
                let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| refused())?;
                if parsed.offset().local_minus_utc() != 0 || !value.ends_with(['Z', 'z']) {
                    return Err(refused());
                }
                let at = parsed.with_timezone(&Utc);
                if at < started_at + millis(MIN_LEASE_TTL_MS)
                    || at > started_at + millis(MAX_PROCESS_TIMEOUT_MS)
                {
                    return Err(refused());
                }
                Ok(Self {
                    at,
                    cause: DeadlineCause::ExpiresAt,
                })
            })
            .transpose()?;
        Ok(match (timeout, expiry) {
            (Some(timeout), Some(expiry)) if expiry.at < timeout.at => Some(expiry),
            (Some(timeout), _) => Some(timeout),
            (None, expiry) => expiry,
        })
    }
}

fn millis(value: u64) -> TimeDelta {
    TimeDelta::milliseconds(i64::try_from(value).unwrap_or(i64::MAX))
}

/// Why a record ended. Exactly one; the first recorded is never overwritten (S3, S11).
///
/// `vanished` (S3) is a reconciliation outcome the daemon records from the store and the cgroup
/// tree after a restart; this driver never observes it, so it never reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedBy {
    Exit,
    Stop,
    LeaseExpiry,
    Timeout,
    ExpiresAt,
}

/// Which stop this is. The daemon records each durably before it calls in (invariant 5).
///
/// The daemon's supervisor clock fires lease expiry and both deadlines, so a daemon passes no
/// [`HostProcessSpec::deadline`] and the driver runs no second clock of its own; the deadline
/// causes are listed here for that clock to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCause {
    /// An explicit stop (S5).
    Stop,
    /// The daemon's lease clock found the lease expired (S4).
    LeaseExpiry,
    /// The daemon's clock reached the `timeout_ms` deadline (S11).
    Timeout,
    /// The daemon's clock reached the `expires_at` deadline (S11).
    ExpiresAt,
}

impl StopCause {
    const fn ended_by(self) -> EndedBy {
        match self {
            Self::Stop => EndedBy::Stop,
            Self::LeaseExpiry => EndedBy::LeaseExpiry,
            Self::Timeout => EndedBy::Timeout,
            Self::ExpiresAt => EndedBy::ExpiresAt,
        }
    }
}

/// Which step of the stop sequence emptied the cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopStep {
    /// Every member was gone within the grace after `SIGTERM`.
    Term,
    /// The grace ran out and `cgroup.kill` ended the rest.
    Kill,
}

/// The main process's exit: a code or the terminating signal, never both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessExit {
    Code(i32),
    Signal(i32),
}

/// A record's end, published once its cgroup is empty and removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcessEnd {
    pub ended_by: EndedBy,
    /// The main process's exit, when the driver observed it.
    pub exit: Option<ProcessExit>,
    /// The stop step that emptied the cgroup; `None` when no member was left to stop.
    pub stop_step: Option<StopStep>,
    pub ended_at: DateTime<Utc>,
}

/// What to run. `env` is the complete environment: nothing of the daemon's own is inherited.
///
/// Its `Debug` form names the environment's variables and never their values (S1): a value is a
/// secret in practice, and `{:?}` is the form a log line or an error message takes.
#[derive(Clone, PartialEq, Eq)]
pub struct HostProcessSpec {
    /// The program and its arguments, executed directly with no shell.
    pub argv: Vec<String>,
    /// Absolute, lexically canonical, an existing directory reached without a symlink.
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    /// The effective deadline, from [`Deadline::effective`].
    pub deadline: Option<Deadline>,
}

impl std::fmt::Debug for HostProcessSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostProcessSpec")
            .field("argv", &self.argv)
            .field("cwd", &self.cwd)
            .field(
                "env",
                &self
                    .env
                    .keys()
                    .map(|name| (name, "<redacted>"))
                    .collect::<BTreeMap<_, _>>(),
            )
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl HostProcessSpec {
    /// [`env_name_is_valid`], as the rule a spec's `env` names must meet: a client checks a
    /// request against the same rule [`HostProcessSpec::validate`] applies, through the type the
    /// port's `start` takes.
    #[must_use]
    pub fn env_name_is_valid(name: &str) -> bool {
        env_name_is_valid(name)
    }

    /// [`check_cwd`], as the rule a spec's `cwd` must meet when the driver opens it.
    ///
    /// # Errors
    ///
    /// `process.cwd-invalid`.
    pub fn check_cwd(cwd: &Path) -> Result<(), DriverError> {
        check_cwd(cwd)
    }

    /// The lexical checks of S1 for `argv`, `env` and `cwd`. Whether `cwd` exists and is reached
    /// without a symlink is decided when it is opened at spawn.
    ///
    /// # Errors
    ///
    /// `process.argv-invalid`, `process.env-invalid` or `process.cwd-invalid`.
    pub fn validate(&self) -> Result<(), DriverError> {
        let argv_bytes = self.argv.iter().map(String::len).sum::<usize>();
        if self.argv.is_empty()
            || self.argv[0].is_empty()
            || self.argv.len() > MAX_PROCESS_ARGV_ITEMS
            || argv_bytes > MAX_PROCESS_ARGV_BYTES
            || self.argv.iter().any(|item| item.contains('\0'))
        {
            return Err(DriverError::refused(
                ARGV_INVALID,
                format!(
                    "argv must be 1..={MAX_PROCESS_ARGV_ITEMS} items, at most \
                     {MAX_PROCESS_ARGV_BYTES} bytes, a non-empty program and no NUL."
                ),
                "argv",
            ));
        }
        let env_bytes = self
            .env
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>();
        if self.env.len() > MAX_PROCESS_ENV_ITEMS
            || env_bytes > MAX_PROCESS_ENV_BYTES
            || self
                .env
                .iter()
                .any(|(name, value)| !valid_env_name(name) || value.contains('\0'))
        {
            return Err(DriverError::refused(
                ENV_INVALID,
                format!(
                    "env must hold at most {MAX_PROCESS_ENV_ITEMS} entries and \
                     {MAX_PROCESS_ENV_BYTES} bytes, names [A-Za-z_][A-Za-z0-9_]* and values \
                     without NUL."
                ),
                "env",
            ));
        }
        if !lexically_canonical(&self.cwd) {
            return Err(cwd_invalid());
        }
        Ok(())
    }
}

/// Whether `name` is an environment name a host process admits: `[A-Za-z_][A-Za-z0-9_]*` (S1).
pub fn env_name_is_valid(name: &str) -> bool {
    valid_env_name(name)
}

/// Whether `cwd` is admitted as a working directory now: absolute, lexically canonical, and an
/// existing directory the kernel reaches without following a symlink (S1). The daemon calls this
/// before it records a start, so a refusal costs no record; [`HostProcessSupervisor::start`]
/// opens the directory again and refuses the same way if it changed in between.
///
/// # Errors
///
/// `process.cwd-invalid`.
#[cfg(target_os = "linux")]
pub fn check_cwd(cwd: &Path) -> Result<(), DriverError> {
    open_cwd(cwd).map(drop)
}

/// Off Linux no host process can start, so no working directory is admitted.
///
/// # Errors
///
/// Always `process.host-unserved`.
#[cfg(not(target_os = "linux"))]
pub fn check_cwd(_cwd: &Path) -> Result<(), DriverError> {
    Err(unserved(format!(
        "Host processes need a Linux cgroup v2 root; this host runs {}.",
        std::env::consts::OS
    )))
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Absolute, with no `.`, `..`, empty component or trailing separator: the spelling
/// `canonicalize` would return for a path without symlinks.
fn lexically_canonical(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute() || bytes.contains(&0) {
        return false;
    }
    if bytes != b"/" && (bytes.ends_with(b"/") || bytes.windows(2).any(|pair| pair == b"//")) {
        return false;
    }
    path.components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
        && !bytes
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
}

fn cwd_invalid() -> DriverError {
    DriverError::refused(
        CWD_INVALID,
        "cwd must be an absolute canonical path to an existing directory reached without a \
         symlink.",
        "cwd",
    )
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;

/// Opens `path` as the working directory: a directory, reached by the kernel without following
/// any symlink in any component (`openat2` with `RESOLVE_NO_SYMLINKS`). The child changes into
/// this descriptor, so a path swapped after the check cannot redirect it.
#[cfg(target_os = "linux")]
pub(crate) fn open_cwd(path: &Path) -> Result<OwnedFd, DriverError> {
    if !lexically_canonical(path) {
        return Err(cwd_invalid());
    }
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| cwd_invalid())?;
    let how = OpenHow {
        flags: u64::try_from(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .expect("open flags are non-negative"),
        mode: 0,
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: the arguments point to an initialized `OpenHow` and a NUL-terminated path.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            &raw const how,
            size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(cwd_invalid());
    }
    // SAFETY: `openat2` returned a new descriptor owned by this scope.
    Ok(unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).expect("descriptor fits i32")) })
}

/// What a started process is. `confinement` is always `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcessStarted {
    pub id: String,
    pub pid: u32,
    /// The cgroup's name beneath the delegated root.
    pub cgroup: String,
    pub started_at: DateTime<Utc>,
    pub deadline: Option<Deadline>,
    pub profile: HostProfile,
    pub confinement: Confinement,
}

/// An observation read at call time (S6). A value the driver could not read is absent, never
/// zero; an ended record has no cgroup and so carries none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcessObservation {
    pub observed_at: DateTime<Utc>,
    /// Cumulative CPU from the cgroup's `cpu.stat` `usage_usec`.
    pub cpu_usec: Option<u64>,
    /// The cgroup's `memory.current`, page cache included; named for what it is, not RSS.
    pub memory_bytes: Option<u64>,
    /// Listening TCP and UDP sockets a member holds.
    pub listening: Option<Vec<ListeningSocket>>,
    pub profile: HostProfile,
    pub confinement: Confinement,
}

fn unserved(message: impl Into<String>) -> DriverError {
    DriverError::unserved(HOST_UNSERVED, message, "process.host")
}

fn not_found() -> DriverError {
    DriverError {
        class: DriverErrorClass::NotFound,
        code: NOT_FOUND,
        message: "No host process has this id.".to_owned(),
        address: Some("process".to_owned()),
        retriable: false,
    }
}

fn cgroup_failed(message: impl Into<String>) -> DriverError {
    DriverError::failed(CGROUP_FAILED, message)
}

/// Re-labels a failure of a shared cgroup primitive, whose codes are exec's, as this family's.
fn as_process(error: DriverError) -> DriverError {
    cgroup_failed(error.message)
}

fn copy_error(error: &DriverError) -> DriverError {
    DriverError {
        class: error.class,
        code: error.code,
        message: error.message.clone(),
        address: error.address.clone(),
        retriable: error.retriable,
    }
}

fn validate_id(id: &str) -> Result<(), DriverError> {
    if id.is_empty()
        || id.len() > MAX_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(DriverError::refused(
            ID_INVALID,
            "A host-process id is 1..=64 bytes of [A-Za-z0-9_-].",
            "process",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
type EndState = Option<Result<HostProcessEnd, DriverError>>;

/// Whether the stop sequence has run, and if so the step that emptied the cgroup.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
enum Termination {
    /// The stop sequence has not completed.
    NotRun,
    /// It ran to completion; `None` when the cgroup was already empty.
    Ran(Option<StopStep>),
}

#[cfg(target_os = "linux")]
struct Entry {
    cgroup: Cgroup,
    cgroup_path: PathBuf,
    /// The main process's pid and a pidfd on it, opened while it was still the driver's unreaped
    /// child. A main process can leave its cgroup (same uid, `exec systemd-run --scope …`); the
    /// pidfd still reaches it, and never a process that reused its number.
    ///
    /// `None` for a record a restarted daemon re-attached by its cgroup
    /// ([`HostProcessSupervisor::reattach`]): its main process was never this run's child, and a
    /// pid read back from anywhere could name another process by now. Membership alone defines
    /// such a record.
    main: Option<(u32, OwnedFd)>,
    log: Arc<ProcessLog>,
    /// The first recorded end cause; never overwritten.
    claim: Mutex<Option<EndedBy>>,
    /// The stop sequence, run at most once to completion; holds the step that emptied the cgroup.
    termination: tokio::sync::Mutex<Termination>,
    end: watch::Sender<EndState>,
}

#[cfg(target_os = "linux")]
impl Entry {
    fn claim(&self, cause: EndedBy) -> EndedBy {
        *self.claim.lock().get_or_insert(cause)
    }

    fn populated(&self) -> Result<bool, DriverError> {
        cgroup_populated(&self.cgroup_path)
    }

    /// Whether the main process has exited, reaped or not, wherever its cgroup is. A re-attached
    /// record has no main process of its own to wait for.
    fn main_exited(&self) -> bool {
        self.main
            .as_ref()
            .is_none_or(|(_, main)| pidfd::exited(main.as_fd()))
    }

    /// Whether anything of the record is left: a member anywhere in its cgroup subtree, or the
    /// main process, which may have left that subtree.
    fn alive(&self) -> Result<bool, DriverError> {
        Ok(self.populated()? || !self.main_exited())
    }

    async fn wait_gone(&self, limit: Duration) -> Result<bool, DriverError> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if !self.alive()? {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(EMPTY_POLL).await;
        }
    }

    /// `signal` to every member of the cgroup subtree, and to the main process once if it is
    /// no longer a member.
    fn signal_record(&self, signal: i32) -> Result<(), DriverError> {
        let reached = tree::signal_members(&self.cgroup_path, signal)?;
        if let Some((pid, main)) = &self.main
            && !reached.contains(pid)
        {
            pidfd::send(main.as_fd(), signal)
                .map_err(|error| cgroup_failed(format!("signal {signal} failed: {error}")))?;
        }
        Ok(())
    }

    /// S3's stop: `SIGTERM` to every member, the bounded grace, then `cgroup.kill`, then wait
    /// until the cgroup is empty. Members of cgroups nested beneath the record's are members,
    /// and the main process is ended even when it left the cgroup. Run once; a later caller gets
    /// the recorded step.
    async fn terminate(&self, grace: Duration) -> Result<Option<StopStep>, DriverError> {
        let mut done = self.termination.lock().await;
        if let Termination::Ran(step) = *done {
            return Ok(step);
        }
        let step = if self.alive()? {
            // A `SIGTERM` that cannot be delivered skips the grace: the stop goes straight to
            // `cgroup.kill`, never leaves the record running.
            let termed = self.signal_record(libc::SIGTERM).is_ok();
            if termed && self.wait_gone(grace).await? {
                Some(StopStep::Term)
            } else {
                // A cgroup removed meanwhile had no member left to kill.
                if let Err(error) = self.cgroup.kill_all()
                    && self.cgroup_path.exists()
                {
                    return Err(as_process(error));
                }
                if let Some((_, main)) = &self.main {
                    pidfd::send(main.as_fd(), libc::SIGKILL).map_err(|error| {
                        cgroup_failed(format!("signal {} failed: {error}", libc::SIGKILL))
                    })?;
                }
                if !self.wait_gone(KILL_SETTLE).await? {
                    return Err(cgroup_failed(
                        "The host-process cgroup did not empty after cgroup.kill.",
                    ));
                }
                Some(StopStep::Kill)
            }
        } else {
            None
        };
        *done = Termination::Ran(step);
        Ok(step)
    }

    async fn ended(&self) -> Result<HostProcessEnd, DriverError> {
        let mut receiver = self.end.subscribe();
        let state = receiver
            .wait_for(Option::is_some)
            .await
            .map_err(|_| cgroup_failed("The host-process supervisor ended without a result."))?;
        match state.as_ref().expect("waited for an end") {
            Ok(end) => Ok(end.clone()),
            Err(error) => Err(copy_error(error)),
        }
    }

    fn current_end(&self) -> EndState {
        match self.end.borrow().as_ref() {
            None => None,
            Some(Ok(end)) => Some(Ok(end.clone())),
            Some(Err(error)) => Some(Err(copy_error(error))),
        }
    }
}

/// Whether the cgroup at `path`, or any cgroup beneath it, has a member. A cgroup already removed
/// has none: the kernel refuses to remove one that still has a member.
#[cfg(target_os = "linux")]
fn cgroup_populated(path: &Path) -> Result<bool, DriverError> {
    let events = match std::fs::read_to_string(path.join("cgroup.events")) {
        Ok(events) => events,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !path.exists() => {
            return Ok(false);
        }
        Err(error) => return Err(cgroup_failed(format!("cgroup.events: {error}"))),
    };
    events
        .lines()
        .find_map(|line| line.strip_prefix("populated "))
        .map(|value| value.trim() != "0")
        .ok_or_else(|| cgroup_failed("cgroup.events omitted populated"))
}

/// What a restarted daemon finds for a record it holds (S3 § restart reconciliation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reattached {
    /// The record's cgroup still has members; the driver supervises it again, by membership.
    Live,
    /// The record's cgroup is gone, or was left empty and is now removed: nothing of it runs.
    Gone,
}

/// The host-process driver over one delegated cgroup root and one owner-private state directory.
#[cfg(target_os = "linux")]
pub struct HostProcessSupervisor {
    cgroup_root: PathBuf,
    state_dir: PathBuf,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
}

#[cfg(target_os = "linux")]
impl std::fmt::Debug for HostProcessSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostProcessSupervisor")
            .field("cgroup_root", &self.cgroup_root)
            .field("state_dir", &self.state_dir)
            .finish_non_exhaustive()
    }
}

#[cfg(target_os = "linux")]
impl HostProcessSupervisor {
    /// Opens the driver, proving the delegated root can hold process cgroups: a probe cgroup is
    /// created, a forked probe process joins it, and both are removed. `state_dir` is created
    /// owner-private (0700) and holds one log per process.
    ///
    /// # Errors
    ///
    /// `process.host-unserved` when `cgroup_root` is absent, is not a cgroup v2 directory, has no
    /// `cgroup.kill` in a child, or cannot be joined from this process. `process.log-failed` when
    /// the state directory cannot be secured.
    pub fn open(cgroup_root: Option<&Path>, state_dir: &Path) -> Result<Arc<Self>, DriverError> {
        let Some(root) = cgroup_root else {
            return Err(unserved(
                "No delegated cgroup v2 root is configured; host processes are not served.",
            ));
        };
        probe_root(root)?;
        std::fs::create_dir_all(state_dir).map_err(|error| state_failed(&error))?;
        let metadata =
            std::fs::symlink_metadata(state_dir).map_err(|error| state_failed(&error))?;
        if !metadata.is_dir() {
            return Err(DriverError::failed(
                "process.log-failed",
                "The host-process state directory is not a directory.",
            ));
        }
        std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| state_failed(&error))?;
        Ok(Arc::new(Self {
            cgroup_root: root.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            entries: Mutex::new(HashMap::new()),
        }))
    }

    fn log_path(&self, id: &str) -> PathBuf {
        self.state_dir.join(format!("{id}.log"))
    }

    fn entry(&self, id: &str) -> Result<Arc<Entry>, DriverError> {
        validate_id(id)?;
        self.entries.lock().get(id).cloned().ok_or_else(not_found)
    }

    /// Starts one host process in a fresh cgroup of its own, placed there before its program's
    /// first instruction.
    ///
    /// The child starts a new session, joins the cgroup by writing `0` to its `cgroup.procs`,
    /// changes into the opened `cwd` and marks every inherited descriptor above standard error
    /// close-on-exec, then executes `argv[0]` directly with exactly `env`. Standard input is
    /// `/dev/null`; standard output and error go, line by line and tagged, to the bounded log.
    ///
    /// # Errors
    ///
    /// The S1 refusals from [`HostProcessSpec::validate`] and [`open_cwd`],
    /// `process.id-invalid` for a malformed or already held id, `process.spawn-failed` when the
    /// program cannot be executed (nothing is left running and the cgroup is removed), and
    /// `process.cgroup-failed` or `process.log-failed` when the host cannot set it up.
    pub fn start(
        self: &Arc<Self>,
        id: &str,
        spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        validate_id(id)?;
        spec.validate()?;
        let cwd = open_cwd(&spec.cwd)?;
        if self.entries.lock().contains_key(id) {
            return Err(DriverError::refused(
                ID_INVALID,
                "This host-process id is already held.",
                "process",
            ));
        }
        let log_path = self.log_path(id);
        let log = Arc::new(ProcessLog::create(&log_path)?);
        let name = format!("{CGROUP_PREFIX}{id}");
        let cgroup_path = self.cgroup_root.join(&name);
        if let Err(error) = std::fs::create_dir(&cgroup_path) {
            let _ = std::fs::remove_file(&log_path);
            return Err(cgroup_failed(format!("cgroup create: {error}")));
        }
        let cgroup = Cgroup::at(cgroup_path.clone(), name.clone());
        let undo = |error: DriverError| {
            let _ = cgroup.prove_empty_and_remove();
            let _ = std::fs::remove_file(&log_path);
            error
        };
        if !cgroup_path.join("cgroup.kill").is_file() {
            return Err(undo(unserved("The delegated cgroup has no cgroup.kill.")));
        }
        let procs = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(cgroup_path.join("cgroup.procs"))
            .map_err(|error| undo(cgroup_failed(format!("cgroup.procs: {error}"))))?;

        let mut command = host_command(spec, procs.as_raw_fd(), cwd.as_raw_fd());
        let started_at = Utc::now();
        let spawned = command.spawn();
        drop(procs);
        drop(cwd);
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                return Err(undo(DriverError::failed(
                    SPAWN_FAILED,
                    format!("host process could not be executed: {error}"),
                )));
            }
        };
        let pid = child.id().unwrap_or(0);
        // The child is unreaped here, so its pid is still its own: the pidfd names exactly it.
        let main = match pidfd::open(pid) {
            Ok(Some(main)) => main,
            failed => {
                let reason = match failed {
                    Err(error) => format!("{error}"),
                    Ok(_) => "the main process has no pid".to_owned(),
                };
                let _ = std::fs::remove_file(&log_path);
                return Err(abandon(&cgroup, cgroup_path, child, &reason));
            }
        };
        let (end, _) = watch::channel(None);
        let entry = Arc::new(Entry {
            cgroup,
            cgroup_path,
            main: Some((pid, main)),
            log: Arc::clone(&log),
            claim: Mutex::new(None),
            termination: tokio::sync::Mutex::new(Termination::NotRun),
            end,
        });
        self.entries
            .lock()
            .insert(id.to_owned(), Arc::clone(&entry));

        let readers = [
            child
                .stdout
                .take()
                .map(|out| tokio::spawn(copy_lines(out, LogStream::Stdout, Arc::clone(&log)))),
            child
                .stderr
                .take()
                .map(|err| tokio::spawn(copy_lines(err, LogStream::Stderr, Arc::clone(&log)))),
        ];
        if let Some(deadline) = spec.deadline {
            tokio::spawn(enforce_deadline(Arc::clone(&entry), deadline));
        }
        tokio::spawn(supervise(entry, child, readers));
        Ok(HostProcessStarted {
            id: id.to_owned(),
            pid,
            cgroup: name,
            started_at,
            deadline: spec.deadline,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }

    /// Re-attaches a record a restarted daemon still holds (S3: "a record whose cgroup still has
    /// members is observed again; a record whose cgroup is gone ends vanished").
    ///
    /// A record this driver already holds answers [`Reattached::Live`] unchanged. Otherwise the
    /// record's cgroup decides: absent, or present and empty (it is then removed), is
    /// [`Reattached::Gone`]; populated, the driver supervises it again by membership alone, with
    /// no main process of its own, and its retained log is reopened. Such a record ends when its
    /// cgroup empties, with no exit status: nobody observed one. Its output pipes did not survive
    /// the restart, so nothing further is logged for it.
    ///
    /// # Errors
    ///
    /// `process.id-invalid`, `process.cgroup-failed` when the cgroup cannot be read or removed,
    /// `process.log-failed` when the retained log cannot be reopened or recreated.
    pub fn reattach(self: &Arc<Self>, id: &str) -> Result<Reattached, DriverError> {
        validate_id(id)?;
        if self.entries.lock().contains_key(id) {
            return Ok(Reattached::Live);
        }
        let name = format!("{CGROUP_PREFIX}{id}");
        let cgroup_path = self.cgroup_root.join(&name);
        match std::fs::symlink_metadata(&cgroup_path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(cgroup_failed("A host-process cgroup is not a directory.")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Reattached::Gone);
            }
            Err(error) => return Err(cgroup_failed(format!("cgroup read: {error}"))),
        }
        if !cgroup_populated(&cgroup_path)? {
            tree::remove(&cgroup_path)?;
            return Ok(Reattached::Gone);
        }
        let log_path = self.log_path(id);
        let log = match std::fs::symlink_metadata(&log_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ProcessLog::create(&log_path)?
            }
            _ => ProcessLog::open_existing(&log_path)?,
        };
        let (end, _) = watch::channel(None);
        let entry = Arc::new(Entry {
            cgroup: Cgroup::at(cgroup_path.clone(), name),
            cgroup_path,
            main: None,
            log: Arc::new(log),
            claim: Mutex::new(None),
            termination: tokio::sync::Mutex::new(Termination::NotRun),
            end,
        });
        self.entries
            .lock()
            .insert(id.to_owned(), Arc::clone(&entry));
        tokio::spawn(supervise_reattached(entry));
        Ok(Reattached::Live)
    }

    /// Stops one process as S3 stops it and waits for its end. The daemon calls this with
    /// [`StopCause::LeaseExpiry`] when its lease clock fires (S4). On a record that already
    /// ended, or whose end was already claimed by an exit or a deadline, it returns that end
    /// unchanged: the first recorded cause stands.
    ///
    /// # Errors
    ///
    /// `process.not-found`, `process.bound-invalid` for a grace above
    /// [`MAX_PROCESS_STOP_GRACE`], or `process.cgroup-failed` when the cgroup cannot be emptied.
    pub async fn stop(
        &self,
        id: &str,
        cause: StopCause,
        grace: Duration,
    ) -> Result<HostProcessEnd, DriverError> {
        if grace > MAX_PROCESS_STOP_GRACE {
            return Err(DriverError::refused(
                BOUND_INVALID,
                "The stop grace exceeds its bound.",
                "grace",
            ));
        }
        let entry = self.entry(id)?;
        if let Some(end) = entry.current_end() {
            return end;
        }
        entry.claim(cause.ended_by());
        entry.terminate(grace).await?;
        entry.ended().await
    }

    /// Begins S3's stop of one process and returns at once: the cause is claimed now (the first
    /// recorded cause stands), and the stop sequence runs in a task this driver owns, as its
    /// supervision does. [`HostProcessSupervisor::ended`] reads the end once it is published. The
    /// daemon's supervisor clock uses this, so a process that ignores `SIGTERM` holds no sweep
    /// for its grace.
    ///
    /// # Errors
    ///
    /// `process.not-found`, or `process.bound-invalid` for a grace above
    /// [`MAX_PROCESS_STOP_GRACE`].
    pub fn begin_stop(
        &self,
        id: &str,
        cause: StopCause,
        grace: Duration,
    ) -> Result<(), DriverError> {
        if grace > MAX_PROCESS_STOP_GRACE {
            return Err(DriverError::refused(
                BOUND_INVALID,
                "The stop grace exceeds its bound.",
                "grace",
            ));
        }
        let entry = self.entry(id)?;
        if entry.current_end().is_some() {
            return Ok(());
        }
        entry.claim(cause.ended_by());
        tokio::spawn(async move {
            let _ = entry.terminate(grace).await;
        });
        Ok(())
    }

    /// Waits for a process's end, however it comes.
    ///
    /// # Errors
    ///
    /// `process.not-found`, or the failure that kept the record from ending with an empty cgroup.
    pub async fn wait(&self, id: &str) -> Result<HostProcessEnd, DriverError> {
        self.entry(id)?.ended().await
    }

    /// The end of a process, if it has ended.
    ///
    /// # Errors
    ///
    /// `process.not-found`.
    pub fn ended(&self, id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        self.entry(id)?.current_end().transpose()
    }

    /// Reads CPU, memory and listening sockets now (S6, wave point 13).
    ///
    /// # Errors
    ///
    /// `process.not-found`. A value that cannot be read is absent in the observation, not an
    /// error.
    pub fn observe(&self, id: &str) -> Result<HostProcessObservation, DriverError> {
        let entry = self.entry(id)?;
        let observed_at = Utc::now();
        let live = entry.current_end().is_none() && entry.cgroup_path.is_dir();
        let (cpu_usec, memory_bytes, listening) = if live {
            (
                entry.cgroup.cpu_usage_micros().ok(),
                read_decimal_counter(&entry.cgroup_path.join("memory.current")).ok(),
                ports::observe(&entry.cgroup_path),
            )
        } else {
            (None, None, None)
        };
        Ok(HostProcessObservation {
            observed_at,
            cpu_usec,
            memory_bytes,
            listening,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }

    /// A bounded tail of one process's log, by bytes, of one stream or both (S7). A log left by
    /// an earlier daemon run is read from the state directory.
    ///
    /// # Errors
    ///
    /// `process.not-found` when neither a record nor a log exists, `process.bound-invalid` for a
    /// budget of zero or above [`MAX_PROCESS_LOG_TAIL_BYTES`], `process.log-failed` when the log
    /// cannot be read.
    pub fn log_tail(
        &self,
        id: &str,
        selection: LogStreamSelection,
        max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        check_tail(id, max_bytes)?;
        let tracked = self
            .entries
            .lock()
            .get(id)
            .map(|entry| Arc::clone(&entry.log));
        if let Some(log) = tracked {
            return log.tail(selection, max_bytes);
        }
        stored_log_tail(&self.state_dir, id, selection, max_bytes)
    }

    /// Forgets an ended record and removes its log (S7: a log is removed with its record).
    ///
    /// # Errors
    ///
    /// `process.not-ended` for a record still live, `process.log-failed` when the log cannot be
    /// removed.
    pub fn forget(&self, id: &str) -> Result<(), DriverError> {
        validate_id(id)?;
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get(id) {
            if entry.current_end().is_none() {
                return Err(DriverError::refused(
                    NOT_ENDED,
                    "A live host process cannot be forgotten.",
                    "process",
                ));
            }
            entries.remove(id);
        }
        drop(entries);
        match std::fs::remove_file(self.log_path(id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DriverError::failed(
                "process.log-failed",
                format!("process log remove: {error}"),
            )),
        }
    }
}

/// The id and budget checks every log read makes, before any file is touched.
fn check_tail(id: &str, max_bytes: u64) -> Result<(), DriverError> {
    validate_id(id)?;
    if max_bytes == 0 || max_bytes > MAX_PROCESS_LOG_TAIL_BYTES {
        return Err(DriverError::refused(
            BOUND_INVALID,
            format!("A log tail is 1..={MAX_PROCESS_LOG_TAIL_BYTES} bytes."),
            "max_bytes",
        ));
    }
    Ok(())
}

/// The tail of a log as the state directory holds it: `process.not-found` when there is none.
/// An existing log is opened without following a symlink; nothing is created.
fn stored_log_tail(
    state_dir: &Path,
    id: &str,
    selection: LogStreamSelection,
    max_bytes: u64,
) -> Result<LogTail, DriverError> {
    let path = state_dir.join(format!("{id}.log"));
    if std::fs::symlink_metadata(&path).is_err() {
        return Err(not_found());
    }
    ProcessLog::open_existing(&path)?.tail(selection, max_bytes)
}

/// The stored logs of one host-process state directory, read without supervising anything (S7).
///
/// A daemon that serves no host process this run still holds the logs an earlier run left beside
/// their records, and `process.logs.get` carries no capability predicate: this reader serves
/// them. It never creates the directory or a log, and runs nothing.
#[derive(Debug)]
pub struct StoredProcessLogs {
    state_dir: PathBuf,
}

impl StoredProcessLogs {
    /// A reader over `state_dir`, which may not exist: then every read is `process.not-found`.
    #[must_use]
    pub fn open(state_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            state_dir: state_dir.to_path_buf(),
        })
    }
}

/// The host-process log port (ADR 0036 S7; `AGENTS.md` invariant 4): the one thing the daemon
/// asks of whatever holds host-process logs. [`ProcessSupervisor`] extends it, and
/// [`StoredProcessLogs`] implements it alone, for a daemon that serves no host process.
pub trait ProcessLogs: Send + Sync {
    /// A bounded tail of one record's log, by bytes, of one stream or both
    /// ([`HostProcessSupervisor::log_tail`]).
    ///
    /// # Errors
    ///
    /// `process.not-found`, `process.id-invalid`, `process.bound-invalid` or
    /// `process.log-failed`.
    fn log_tail(
        &self,
        id: &str,
        selection: LogStreamSelection,
        max_bytes: u64,
    ) -> Result<LogTail, DriverError>;
}

impl ProcessLogs for StoredProcessLogs {
    fn log_tail(
        &self,
        id: &str,
        selection: LogStreamSelection,
        max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        check_tail(id, max_bytes)?;
        stored_log_tail(&self.state_dir, id, selection, max_bytes)
    }
}

/// The host-process driver port (ADR 0036; `AGENTS.md` invariant 4): everything the daemon asks
/// of a host-process driver, and nothing of how one is built.
///
/// [`HostProcessSupervisor`] implements it over a delegated cgroup v2 root; only the composition
/// root names that type. A client holds an `Arc<dyn ProcessSupervisor>` and names nothing but
/// this trait and the types its signatures force. Its logs are read through [`ProcessLogs`].
///
/// Every stop runs the driver's bounded default grace, [`DEFAULT_PROCESS_STOP_GRACE`], between
/// `SIGTERM` and `cgroup.kill`; a client chooses no grace of its own, and no call waits for a
/// stop's end: [`ProcessSupervisor::begin_stop`] begins it and [`ProcessSupervisor::ended`]
/// reads it.
pub trait ProcessSupervisor: ProcessLogs {
    /// Starts `spec` as the record `id` ([`HostProcessSupervisor::start`]).
    ///
    /// # Errors
    ///
    /// The S1 refusals, `process.id-invalid`, `process.spawn-failed`, `process.cgroup-failed` or
    /// `process.log-failed`.
    fn start(
        self: Arc<Self>,
        id: &str,
        spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError>;

    /// Supervises a record an earlier daemon run left again, by its cgroup
    /// ([`HostProcessSupervisor::reattach`]).
    ///
    /// # Errors
    ///
    /// `process.id-invalid`, `process.cgroup-failed` or `process.log-failed`.
    fn reattach(self: Arc<Self>, id: &str) -> Result<Reattached, DriverError>;

    /// Claims `cause` and begins the stop sequence, returning at once
    /// ([`HostProcessSupervisor::begin_stop`]); [`ProcessSupervisor::ended`] reads the end.
    ///
    /// # Errors
    ///
    /// `process.not-found`.
    fn begin_stop(&self, id: &str, cause: StopCause) -> Result<(), DriverError>;

    /// The end of a record, once published ([`HostProcessSupervisor::ended`]).
    ///
    /// # Errors
    ///
    /// `process.not-found`.
    fn ended(&self, id: &str) -> Result<Option<HostProcessEnd>, DriverError>;

    /// CPU, memory and listening sockets read now ([`HostProcessSupervisor::observe`]).
    ///
    /// # Errors
    ///
    /// `process.not-found`.
    fn observe(&self, id: &str) -> Result<HostProcessObservation, DriverError>;

    /// Forgets an ended record and removes its log ([`HostProcessSupervisor::forget`]).
    ///
    /// # Errors
    ///
    /// `process.not-ended` or `process.log-failed`.
    fn forget(&self, id: &str) -> Result<(), DriverError>;
}

#[cfg(target_os = "linux")]
impl ProcessLogs for HostProcessSupervisor {
    fn log_tail(
        &self,
        id: &str,
        selection: LogStreamSelection,
        max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        Self::log_tail(self, id, selection, max_bytes)
    }
}

#[cfg(target_os = "linux")]
impl ProcessSupervisor for HostProcessSupervisor {
    fn start(
        self: Arc<Self>,
        id: &str,
        spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        Self::start(&self, id, spec)
    }

    fn reattach(self: Arc<Self>, id: &str) -> Result<Reattached, DriverError> {
        Self::reattach(&self, id)
    }

    fn begin_stop(&self, id: &str, cause: StopCause) -> Result<(), DriverError> {
        Self::begin_stop(self, id, cause, DEFAULT_PROCESS_STOP_GRACE)
    }

    fn ended(&self, id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        Self::ended(self, id)
    }

    fn observe(&self, id: &str) -> Result<HostProcessObservation, DriverError> {
        Self::observe(self, id)
    }

    fn forget(&self, id: &str) -> Result<(), DriverError> {
        Self::forget(self, id)
    }
}

#[cfg(target_os = "linux")]
fn state_failed(error: &std::io::Error) -> DriverError {
    DriverError::failed(
        "process.log-failed",
        format!("host-process state directory: {error}"),
    )
}

/// The command for `spec`: `argv` with no shell, exactly `env`, standard input from `/dev/null`,
/// both outputs piped, and [`child_setup`] between fork and exec. `procs_fd` and `cwd_fd` must
/// stay open until the spawn returns.
#[cfg(target_os = "linux")]
fn host_command(spec: &HostProcessSpec, procs_fd: i32, cwd_fd: i32) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&spec.argv[0]);
    command
        .args(&spec.argv[1..])
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    // SAFETY: the closure runs in the forked child before exec and calls only
    // async-signal-safe entry points (setsid, write, fchdir, close_range) on descriptor
    // numbers the parent holds open across the fork; it allocates nothing.
    unsafe {
        command.pre_exec(move || child_setup(procs_fd, cwd_fd));
    }
    command
}

/// Ends a started process the driver cannot supervise: nothing may be left running. Kills the
/// whole cgroup and the child, reaps the child, and removes the cgroup once it is empty.
#[cfg(target_os = "linux")]
fn abandon(
    cgroup: &Cgroup,
    cgroup_path: PathBuf,
    mut child: tokio::process::Child,
    reason: &str,
) -> DriverError {
    let _ = cgroup.kill_all();
    let _ = child.start_kill();
    tokio::spawn(async move {
        let _ = child.wait().await;
        for _ in 0..100 {
            if tree::remove(&cgroup_path).is_ok() {
                break;
            }
            tokio::time::sleep(EMPTY_POLL).await;
        }
    });
    cgroup_failed(format!(
        "The host process could not be pinned by a pidfd: {reason}"
    ))
}

/// The child's setup between fork and exec. Async-signal-safe only.
#[cfg(target_os = "linux")]
fn child_setup(procs_fd: i32, cwd_fd: i32) -> std::io::Result<()> {
    // SAFETY: each call takes plain integers or a static byte; none allocates.
    unsafe {
        if libc::setsid() < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Writing `0` moves the calling process: the child is in its cgroup before exec, so
        // nothing it runs or forks can start outside it.
        if libc::write(procs_fd, b"0".as_ptr().cast(), 1) != 1 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fchdir(cwd_fd) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // No daemon descriptor survives into the program. CLOEXEC rather than close, so the
        // spawn's own error pipe still reports a failed exec.
        if libc::syscall(
            libc::SYS_close_range,
            3_u32,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Proves `root` can hold host-process cgroups, or refuses by name.
#[cfg(target_os = "linux")]
fn probe_root(root: &Path) -> Result<(), DriverError> {
    let statfs = nix::sys::statfs::statfs(root)
        .map_err(|error| unserved(format!("cgroup root: {error}")))?;
    if statfs.filesystem_type() != nix::sys::statfs::CGROUP2_SUPER_MAGIC {
        return Err(unserved(
            "The configured cgroup root is not a cgroup v2 directory.",
        ));
    }
    let probe = root.join(format!("{CGROUP_PREFIX}probe-{}", ulid::Ulid::generate()));
    std::fs::create_dir(&probe)
        .map_err(|error| unserved(format!("cgroup root is not writable: {error}")))?;
    let result = (|| {
        if !probe.join("cgroup.kill").is_file() {
            return Err(unserved("The delegated cgroup has no cgroup.kill."));
        }
        let procs = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(probe.join("cgroup.procs"))
            .map_err(|error| unserved(format!("cgroup.procs: {error}")))?;
        if join_from_forked_child(procs.as_raw_fd()) {
            Ok(())
        } else {
            Err(unserved(
                "A process forked by this daemon cannot join a cgroup under the delegated root.",
            ))
        }
    })();
    let mut removed = false;
    for _ in 0..50 {
        if std::fs::remove_dir(&probe).is_ok() {
            removed = true;
            break;
        }
        std::thread::sleep(EMPTY_POLL);
    }
    result?;
    if removed {
        Ok(())
    } else {
        Err(unserved("The probe cgroup could not be removed."))
    }
}

/// Forks a child that only writes `0` to `procs_fd` and exits with whether it could.
#[cfg(target_os = "linux")]
fn join_from_forked_child(procs_fd: i32) -> bool {
    // SAFETY: the child calls only `write` and `_exit`, which are async-signal-safe, on a
    // descriptor the parent opened before the fork; it never returns into Rust code.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: as above.
        unsafe {
            let joined = libc::write(procs_fd, b"0".as_ptr().cast(), 1) == 1;
            libc::_exit(i32::from(!joined));
        }
    }
    if pid < 0 {
        return false;
    }
    let mut status = 0;
    loop {
        // SAFETY: waits for the one child this function forked.
        let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        if waited == pid {
            break;
        }
        if waited < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

/// Copies one standard stream into the log, a line at a time. A line longer than the log's line
/// bound is stored cut at it with its full length; a final line without a newline is stored too.
#[cfg(target_os = "linux")]
async fn copy_lines<R: AsyncRead + Unpin>(mut reader: R, stream: LogStream, log: Arc<ProcessLog>) {
    let max_line = log.max_line();
    let mut chunk = vec![0_u8; 8_192];
    let mut line = Vec::with_capacity(256);
    let mut full_len = 0_u64;
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                let _ = log.append_line(stream, &line, Some(full_len));
                line.clear();
                full_len = 0;
                continue;
            }
            full_len += 1;
            if line.len() < max_line {
                line.push(*byte);
            }
        }
    }
    if full_len > 0 {
        let _ = log.append_line(stream, &line, Some(full_len));
    }
}

/// Fires the S11 deadline: claims its cause and runs the stop sequence, unless the record ended
/// first.
#[cfg(target_os = "linux")]
async fn enforce_deadline(entry: Arc<Entry>, deadline: Deadline) {
    let wait = (deadline.at - Utc::now())
        .to_std()
        .unwrap_or(Duration::ZERO);
    let mut receiver = entry.end.subscribe();
    tokio::select! {
        () = tokio::time::sleep(wait) => {}
        _ = receiver.wait_for(Option::is_some) => return,
    }
    entry.claim(match deadline.cause {
        DeadlineCause::Timeout => EndedBy::Timeout,
        DeadlineCause::ExpiresAt => EndedBy::ExpiresAt,
    });
    let _ = entry.terminate(DEFAULT_PROCESS_STOP_GRACE).await;
}

/// Owns the main process: reaps it, ends whatever members remain, drains the log, removes the
/// cgroup and publishes the end. A record ends only with an empty, removed cgroup (S3).
#[cfg(target_os = "linux")]
async fn supervise(
    entry: Arc<Entry>,
    mut child: tokio::process::Child,
    readers: [Option<tokio::task::JoinHandle<()>>; 2],
) {
    let status = child.wait().await.ok();
    let exit = status.and_then(|status| {
        status
            .code()
            .map(ProcessExit::Code)
            .or_else(|| status.signal().map(ProcessExit::Signal))
    });
    let ended_by = entry.claim(EndedBy::Exit);
    let result = async {
        let stop_step = entry.terminate(DEFAULT_PROCESS_STOP_GRACE).await?;
        for reader in readers.into_iter().flatten() {
            let _ = tokio::time::timeout(DRAIN_SETTLE, reader).await;
        }
        let mut removed = Err(cgroup_failed(
            "The host-process cgroup could not be removed.",
        ));
        for _ in 0..100 {
            // Members may have created cgroups beneath the record's; once the subtree is
            // unpopulated they are removed with it, children first.
            removed = if entry.populated()? {
                Err(cgroup_failed(
                    "The host-process cgroup still has members after its stop.",
                ))
            } else {
                tree::remove(&entry.cgroup_path)
            };
            if removed.is_ok() {
                break;
            }
            tokio::time::sleep(EMPTY_POLL).await;
        }
        removed?;
        Ok(HostProcessEnd {
            ended_by,
            exit,
            stop_step,
            ended_at: Utc::now(),
        })
    }
    .await;
    entry.end.send_replace(Some(result));
}

/// How often a re-attached record's cgroup is read for its emptying: it has no main process
/// whose exit could wake the driver.
#[cfg(target_os = "linux")]
const REATTACHED_POLL: Duration = Duration::from_millis(100);

/// Owns a re-attached record: waits for its cgroup to empty, by its own exit or by a stop, then
/// removes the cgroup and publishes the end with no exit status (nobody observed one).
#[cfg(target_os = "linux")]
async fn supervise_reattached(entry: Arc<Entry>) {
    let result = async {
        while entry.populated()? {
            tokio::time::sleep(REATTACHED_POLL).await;
        }
        let ended_by = entry.claim(EndedBy::Exit);
        // A stop that emptied the cgroup holds the step; otherwise none ran.
        let stop_step = entry.terminate(DEFAULT_PROCESS_STOP_GRACE).await?;
        let mut removed = Err(cgroup_failed(
            "The host-process cgroup could not be removed.",
        ));
        for _ in 0..100 {
            removed = if entry.populated()? {
                Err(cgroup_failed(
                    "The host-process cgroup still has members after its stop.",
                ))
            } else {
                tree::remove(&entry.cgroup_path)
            };
            if removed.is_ok() {
                break;
            }
            tokio::time::sleep(EMPTY_POLL).await;
        }
        removed?;
        Ok(HostProcessEnd {
            ended_by,
            exit: None,
            stop_step,
            ended_at: Utc::now(),
        })
    }
    .await;
    entry.end.send_replace(Some(result));
}
