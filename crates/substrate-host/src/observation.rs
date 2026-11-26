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
#[non_exhaustive]
pub struct EndOfRun {
    /// `memory.events` `oom_kill`. `None` where the counter could not be read: absent, never
    /// zero (invariant 3).
    pub memory_oom_kills: Option<u64>,
    /// `pids.events` `max` of the exec's cgroup, read once the leader exited (design 36 § 3).
    /// Where the cgroup has no `pids.events.local`, the kernel counts there every creation
    /// refused for a process of the cgroup, also by the bound of a cgroup above it; where it has
    /// that file and the cgroup2 mount has no `pids_localevents`, the hits of the cgroup's own
    /// bound and of any bound beneath it (design 36 D10). `None` where the counter could not be
    /// read: absent, never zero (invariant 3).
    pub process_limit_hits: Option<u64>,
    /// The live output queue stalled after the child had exited on its own: the record still
    /// carries `session.output-backpressure`, and the stall ended nothing (design 36 D5).
    pub output_stall_after_exit: bool,
    /// The driver recorded `exec.aperture-byte-limit` for the exec: an egress aperture's byte
    /// ceiling was reached. Read where a stall after the exit replaced that refusal on the record
    /// (design 36 D4, D5).
    pub egress_ceiling_reached: bool,
    /// What the exec used (design 36 D38): the wall time from the start to the leader's exit
    /// (D32), and the CPU time, peak memory and peak process count of the exec's cgroup, from the
    /// terminal sample of a measured exec (D33). Each figure that could not be read is `None`:
    /// absent, never zero (invariant 3). Reading it refuses nothing (D34).
    pub usage: crate::confinement::RunUsage,
}
