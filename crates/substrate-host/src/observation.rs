//! What an exec reports, named by the driver contract on every platform.

use substrate_wire::Exec;

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
    pub output_complete: bool,
    pub cgroup: Option<String>,
    pub leader_pid: Option<u32>,
}
