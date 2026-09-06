//! Explicit immutable node-profile installation, independent of the application entrypoint.

#[path = "../container_profiles.rs"]
mod profiles;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Verify only; performs no profile installation or filesystem mutation.
    #[arg(long, conflicts_with = "hold")]
    check: bool,
    /// Remain available for deployment readiness checks until SIGINT/SIGTERM.
    #[arg(long)]
    hold: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.check {
        return profiles::check();
    }
    // Register before installation so PID 1 honors normal deployment termination throughout.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    profiles::install()?;
    println!("versioned container execution profiles installed and enforced");
    if args.hold {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = terminate.recv() => break,
                result = tokio::signal::ctrl_c() => { result?; break; },
                _ = interval.tick() => profiles::check()?,
            }
        }
    }
    Ok(())
}
