#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

fn main() -> std::io::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let procs = arguments.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing cgroup.procs path",
        )
    })?;
    let program = arguments
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing program"))?;
    let program_arguments: Vec<OsString> = arguments.collect();
    std::fs::write(procs, b"0")?;
    Err(Command::new(program).args(program_arguments).exec())
}
