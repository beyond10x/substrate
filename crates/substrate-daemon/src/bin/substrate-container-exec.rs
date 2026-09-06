//! Explicit PID-1 bootstrap for the container-private host execution profile.

#[path = "../container_bootstrap.rs"]
mod bootstrap;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(
    about = "Prepare a private container delegation and start the non-root daemon",
    version
)]
struct Args {
    /// Select the image's existing `SYS_ADMIN` file-capability quota executable.
    #[arg(long)]
    project_quotas: bool,
    /// Normal daemon arguments. The cgroup root is fixed by this entrypoint.
    #[arg(last = true, required = true)]
    daemon_arguments: Vec<OsString>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match bootstrap::run(args.project_quotas, args.daemon_arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("container execution bootstrap refused: {error:#}");
            ExitCode::FAILURE
        }
    }
}
