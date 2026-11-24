//! What an exec reports, named by the driver contract on every platform.

use substrate_wire::Exec;

use crate::output_retention::KeptOutput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeFrame {
    pub stream: PipeStream,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecObservation {
    pub resource: Exec,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Which bytes of stdout `stdout` holds (design 29 § 6). Absent where the observation was not
    /// made by the host driver's drain, such as one rebuilt from a stored record.
    pub stdout_kept: Option<KeptOutput>,
    /// Which bytes of stderr `stderr` holds; absent as `stdout_kept` is.
    pub stderr_kept: Option<KeptOutput>,
    pub output_complete: bool,
    pub cgroup: Option<String>,
    pub leader_pid: Option<u32>,
    /// What the host driver read from the exec's own cgroup when the exec ended (design 36 D9).
    /// `Some` only on an observation whose end the host driver recorded; `None` while the exec
    /// runs, on an observation rebuilt from a stored record and on one another driver made.
    pub end_of_run: Option<EndOfRun>,
}

/// What the host driver read from the exec's own cgroup when the exec ended, after its leader
/// exited and before the cgroup was removed (design 36 D9). Host-internal: no wire member
/// carries it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EndOfRun {
    /// `memory.events` `oom_kill`. `None` where the counter could not be read: absent, never
    /// zero (invariant 3).
    pub memory_oom_kills: Option<u64>,
}
