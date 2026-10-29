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
mod chart;
mod chart_cluster;
mod chart_yaml;
mod container_profiles;
mod docs;
mod ess;
mod image_startup;
mod json;
mod lane_record;
mod licenses;
mod links;
mod mcp;
mod mcp_smoke;
mod namespace_lane;
mod namespace_lane_observe;
mod namespace_lane_render;
mod package;
mod packages;
mod quota_fixture;
mod quota_lane;
mod render;
mod render_v2;
mod repo;
mod report;
mod secrets;
mod site_drift;
mod site_ux;
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
    /// Check the Kubernetes chart (static) or run its kind scenarios (cluster), per ADR 0046.
    #[command(name = "chart-lane")]
    ChartLane(chart::LaneArgs),
    /// Check the namespace lane's objects (check) or run its first observations on kind (observe).
    #[command(name = "namespace-lane")]
    NamespaceLane(namespace_lane::Args),
    /// Package the released Kubernetes chart with its version and pinned daemon digest.
    #[command(name = "package-chart")]
    PackageChart(chart::PackageArgs),
    /// Validate the standalone public documentation.
    CheckDocs,
    /// Validate ESS and reject drift in the committed conformance suite.
    CheckEss,
    /// Check the built documentation site and bind it to its source commit.
    DocsProvenance(docs::ProvenanceArgs),
    /// Regenerate the release notes page from the newest released CHANGELOG.md section.
    DocsReleases,
    /// Write the public site's measured UX values as its new ratchet baseline.
    DocsBaseline(site_ux::BaselineArgs),
    /// Verify final daemon image files and non-root startup capability masks on local Linux Docker.
    #[command(name = "check-image-startup")]
    ImageStartup(image_startup::Args),
    /// Reject dependencies affected by a current `RustSec` vulnerability.
    #[command(name = "check-advisories")]
    Advisories,
    /// Verify workspace licensing and the locked graph's deterministic third-party notices.
    #[command(name = "check-licenses")]
    Licenses,
    /// Refuse publishable members and verify the runtime source-package boundary.
    #[command(name = "check-packages")]
    Packages,
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
    /// Verify the private MCP adapter stays behind the SDK and bounded stdio boundary.
    #[command(name = "check-mcp-boundary")]
    McpBoundary,
    /// Run the manual credentialed Codex smoke against a shipped MCP binary.
    #[command(name = "mcp-codex-smoke")]
    McpCodexSmoke(mcp_smoke::Args),
    /// Run the ignored real project-quota cases, or refuse by name when their fixture is absent.
    #[command(name = "quota-lane")]
    QuotaLane(quota_lane::Args),
    /// Provision the project-quota fixture in the checked-in container recipe and run the lane there.
    #[command(name = "quota-fixture")]
    QuotaFixture(quota_fixture::Args),
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
    /// Verify released bundle directories concurrently with a bounded worker count.
    #[command(name = "check-bundles")]
    CheckBundles(bundle::ManyArgs),
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
    // Each arm ends in its own exit code: a check prints its own report there, so no verb's
    // outcome shares a binding with another verb's report.
    match cli.command {
        Command::ChartLane(args) => chart::run_lane(&args),
        Command::NamespaceLane(args) => namespace_lane::run(&args),
        Command::PackageChart(args) => chart::run_package(&args),
        Command::CheckEss => Ok(ess::check(&repo::root()?)?.emit()),
        Command::CheckDocs => Ok(docs::check(&repo::root()?)?.emit()),
        Command::DocsProvenance(args) => docs::provenance(&args),
        Command::DocsReleases => docs::write_release_notes(),
        Command::DocsBaseline(args) => site_ux::write_baseline(&args),
        Command::ImageStartup(args) => image_startup::run(&args),
        Command::Advisories => Ok(advisories::check(&repo::root()?)?.emit()),
        Command::Licenses => Ok(licenses::check(&repo::root()?)?.emit()),
        Command::Packages => Ok(packages::check_with_executor_tree(&repo::root()?)?.emit()),
        Command::Secrets => Ok(secrets::check(&repo::root()?)?.emit()),
        Command::BotFiles(args) => Ok(bot_files::check(&args).emit()),
        Command::Toolchain { root } => {
            let root = match root {
                Some(root) => root,
                None => repo::root()?,
            };
            Ok(toolchain::check(&root).emit())
        }
        Command::Links => Ok(links::check(&repo::root()?)?.emit()),
        Command::McpBoundary => Ok(mcp::check(&repo::root()?)?.emit()),
        Command::McpCodexSmoke(args) => mcp_smoke::run(&args),
        Command::QuotaLane(args) => quota_lane::run(&args),
        Command::QuotaFixture(args) => quota_fixture::run(&args),
        Command::Adrs => Ok(adrs::check(&repo::root()?)?.emit()),
        Command::CheckJson(args) => json::run(&args),
        Command::PackageBundle(args) => package::run(&args),
        Command::CheckBundle(args) => bundle::run(&args),
        Command::CheckBundles(args) => bundle::run_many(&args),
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
            if generator == "xtask/src/render_v2.rs" {
                render_v2::run(&args)
            } else {
                render::run(&args)
            }
        }
    }
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
