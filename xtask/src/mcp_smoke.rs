//! Manual Codex smoke for the disposable MCP adapter; credentials remain owned by Codex.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context as _, Result, bail};
use clap::Args as ClapArgs;
use ulid::Ulid;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Shipped substrate-mcp binary Codex should launch over stdio.
    #[arg(long, value_name = "PATH")]
    server: PathBuf,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let server = std::fs::canonicalize(&args.server)
        .with_context(|| format!("resolving MCP server {}", args.server.display()))?;
    let metadata = std::fs::metadata(&server)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        bail!("MCP server must be an executable regular file");
    }
    let create = Ulid::generate();
    let write = Ulid::generate();
    let execute = Ulid::generate();
    let retire = Ulid::generate();
    let destroy = Ulid::generate();
    let prompt = format!(
        "Use only the substrate MCP tools for this conformance smoke. Call machine_get. Create an \
         empty workspace with operation_id {create}. Write the UTF-8 bytes 'hello Codex MCP' to \
         input.txt with operation_id {write}. Start argv \
         ['/usr/bin/sha256sum','/workspace/input.txt'] with operation_id {execute}, timeout_ms 5000, \
         cpu_millis 5000, memory_bytes 67108864, processes 4, and output_bytes 4096. If execution \
         is refused, report the exact refusal code and reconcile {execute} with operation_get. If \
         it starts, wait, read stdout, request metrics only when machine facts prove them, then \
         retire it with operation_id {retire}. Destroy the workspace with operation_id {destroy}. \
         Return a short table of exact observations; do not use shell commands or repository files."
    );
    let quoted_server = toml::Value::String(server.display().to_string()).to_string();
    let status = Command::new("codex")
        .args([
            "exec",
            "--ephemeral",
            "--json",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "-c",
            &format!("mcp_servers.substrate.command={quoted_server}"),
            "-c",
            "mcp_servers.substrate.required=true",
            "-c",
            "mcp_servers.substrate.tool_timeout_sec=35",
            "-c",
            "mcp_servers.substrate.default_tools_approval_mode=\"approve\"",
            &prompt,
        ])
        .stdin(Stdio::null())
        .status()
        .context("starting credentialed Codex CLI smoke")?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
