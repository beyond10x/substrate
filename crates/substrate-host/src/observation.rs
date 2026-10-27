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
}
