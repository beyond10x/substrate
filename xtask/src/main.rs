//! Repository tooling for the substrate workspace.
//!
//! `cargo xtask <verb>` runs the gate's own checks. They were Python scripts under `scripts/`
//! until `story:tooling-moves-to-cargo-xtask`; anything that runs in a b10x foundation repository
//! is Rust (`atlas/AGENTS.md`, section *Language*). The four frozen `render-contract-bundle*.py`
//! and `check-contract-bundle*.py` pairs stay Python: they are the reproducibility proof of the
//! frozen bundles `0.1.0`–`0.4.0` (AGENTS.md invariant 6), not tooling.

mod adrs;
mod advisories;
mod bot_files;
mod bundle;
mod json;
mod licenses;
mod links;
mod package;
mod render;
mod render_v2;
mod repo;
mod report;
mod secrets;
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
    /// Reject dependencies affected by a current `RustSec` vulnerability.
    #[command(name = "check-advisories")]
    Advisories,
    /// Verify workspace licensing and the locked graph's deterministic third-party notices.
    #[command(name = "check-licenses")]
    Licenses,
    /// Scan every reachable commit with the checksum-pinned Gitleaks release.
    #[command(name = "check-secrets")]
    Secrets,
    /// Reject bot configuration or key files with unsafe Unix metadata.
    #[command(name = "check-bot-files")]
    BotFiles(bot_files::Args),
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
    /// Reject contract JSON that no bundled schema classifies, or that its schema rejects.
    #[command(name = "check-json")]
    CheckJson(json::Args),
    /// Package a released contract bundle as a deterministic OCI image layout.
    #[command(name = "package-bundle")]
    PackageBundle(package::Args),
    /// Verify a released contract bundle directory against its authored source.
    #[command(name = "check-bundle")]
    CheckBundle(bundle::Args),
    /// Render a contract bundle from substrate-wire and its authored source tree.
    #[command(name = "render-bundle")]
    RenderBundle(render::Args),
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
        Command::Advisories => advisories::check(&repo::root()?)?,
        Command::Licenses => licenses::check(&repo::root()?)?,
        Command::Secrets => secrets::check(&repo::root()?)?,
        Command::BotFiles(args) => bot_files::check(&args),
        Command::Toolchain { root } => {
            let root = match root {
                Some(root) => root,
                None => repo::root()?,
            };
            toolchain::check(&root)
        }
        Command::Links => links::check(&repo::root()?)?,
        Command::Adrs => adrs::check(&repo::root()?)?,
        Command::CheckJson(args) => return json::run(&args),
        Command::PackageBundle(args) => return package::run(&args),
        Command::CheckBundle(args) => return bundle::run(&args),
        Command::RenderBundle(args) => {
            let root = repo::root()?;
            let source = args
                .source
                .clone()
                .unwrap_or_else(|| root.join("xtask/bundle-source"));
            let authored_bundle = source.join(&args.version).join("documents/bundle.json");
            let document: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&authored_bundle)?)?;
            let generator = document
                .pointer("/generator/name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            return if generator == "xtask/src/render_v2.rs" {
                render_v2::run(&args)
            } else {
                render::run(&args)
            };
        }
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
