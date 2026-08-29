//! Repository tooling for the substrate workspace.
//!
//! `cargo xtask <verb>` runs the gate's own checks. They were Python scripts under `scripts/`
//! until `story:tooling-moves-to-cargo-xtask`; anything that runs in a b10x foundation repository
//! is Rust (`atlas/AGENTS.md`, section *Language*). The four frozen `render-contract-bundle*.py`
//! and `check-contract-bundle*.py` pairs stay Python: they are the reproducibility proof of the
//! frozen bundles `0.1.0`–`0.4.0` (AGENTS.md invariant 6), not tooling.

mod adrs;
mod links;
mod package;
mod repo;
mod report;
mod toolchain;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// The repository's own checks, as `cargo xtask <verb>`.
#[derive(Debug, Parser)]
#[command(name = "xtask", about = "substrate repository tooling", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Reject a Rust toolchain version that the three pinning files do not agree on.
    #[command(name = "check-toolchain")]
    Toolchain {
        /// Repository root to check (default: the repository this binary belongs to).
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Reject machine-local Markdown links and broken repository-relative targets.
    #[command(name = "check-links")]
    Links,
    /// Validate ADR identity, frontmatter, index agreement, and supersession links.
    #[command(name = "check-adrs")]
    Adrs,
    /// Package a released contract bundle as a deterministic OCI image layout.
    #[command(name = "package-bundle")]
    PackageBundle(package::Args),
}

fn main() -> ExitCode {
    match dispatch() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("xtask: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch() -> Result<ExitCode> {
    let cli = Cli::parse();
    let report = match cli.command {
        Command::Toolchain { root } => {
            let root = match root {
                Some(root) => root,
                None => repo::root()?,
            };
            toolchain::check(&root)
        }
        Command::Links => links::check(&repo::root()?)?,
        Command::Adrs => adrs::check(&repo::root()?)?,
        Command::PackageBundle(args) => return package::run(&args),
    };
    Ok(report.emit())
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }
}
