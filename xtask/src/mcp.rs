//! The disposable MCP adapter is a private SDK composition, not another daemon ingress.

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::report::Report;

#[allow(
    clippy::too_many_lines,
    reason = "one gate keeps the complete private dependency and transport boundary visible"
)]
pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    let manifest_path = root.join("crates/substrate-mcp/Cargo.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&manifest_text)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    if manifest
        .get("package")
        .and_then(|value| value.get("publish"))
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        failures.push("crates/substrate-mcp must set publish = false".to_owned());
    }
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .context("substrate-mcp dependencies are absent")?;
    for (name, dependency) in dependencies {
        let package = dependency
            .as_table()
            .and_then(|fields| fields.get("package"))
            .and_then(toml::Value::as_str)
            .unwrap_or(name);
        if package.contains("substrate") && package != "b10x-substrate-sdk" {
            failures.push(format!(
                "crates/substrate-mcp has forbidden direct Substrate dependency {name} ({package})"
            ));
        }
        if matches!(
            package,
            "axum" | "hyper" | "hyper-util" | "reqwest" | "tokio-tungstenite" | "tower"
        ) {
            failures.push(format!(
                "crates/substrate-mcp has forbidden remote-transport dependency {name} ({package})"
            ));
        }
    }
    let sdk = dependencies
        .get("b10x-substrate-sdk")
        .and_then(toml::Value::as_table)
        .context("substrate-mcp must declare the SDK as a dependency table")?;
    if !sdk
        .get("version")
        .and_then(toml::Value::as_str)
        .is_some_and(|version| version.starts_with('='))
        || sdk.get("features").and_then(toml::Value::as_array)
            != Some(&vec![toml::Value::String("linked-daemon".to_owned())])
    {
        failures
            .push("substrate-mcp must exact-pin the SDK and enable only linked-daemon".to_owned());
    }
    let rmcp = dependencies
        .get("rmcp")
        .and_then(toml::Value::as_table)
        .context("substrate-mcp must declare rmcp as a dependency table")?;
    if rmcp.get("workspace").and_then(toml::Value::as_bool) != Some(true) {
        failures.push("substrate-mcp must take the exact workspace rmcp pin".to_owned());
    }
    let workspace_text = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let workspace: toml::Value = toml::from_str(&workspace_text)?;
    let rmcp_workspace = workspace
        .get("workspace")
        .and_then(|value| value.get("dependencies"))
        .and_then(|value| value.get("rmcp"))
        .and_then(toml::Value::as_table)
        .context("workspace rmcp dependency is absent")?;
    if rmcp_workspace
        .get("default-features")
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        failures.push("workspace rmcp dependency must disable default features".to_owned());
    }
    if rmcp_workspace.get("features").is_some() {
        failures.push(
            "workspace rmcp dependency must enable no transport or service features".to_owned(),
        );
    }
    if !rmcp_workspace
        .get("version")
        .and_then(toml::Value::as_str)
        .is_some_and(|version| version.starts_with('='))
    {
        failures.push("workspace rmcp dependency must be exact-version pinned".to_owned());
    }

    let mut pending = vec![root.join("crates/substrate-mcp/src")];
    while let Some(source_root) = pending.pop() {
        for entry in std::fs::read_dir(&source_root)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path)?;
            for forbidden in [
                "rmcp::transport",
                "transport::stdio",
                "serve_server",
                "serve_client",
                "streamable_http",
                "StreamableHttp",
                "oauth",
                "OAuth",
                "TcpListener",
                "UdpSocket",
            ] {
                if source.contains(forbidden) {
                    failures.push(format!(
                        "{} selects forbidden MCP surface {forbidden}",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    ));
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(Report::passed(
            "private MCP adapter is SDK-only with an exact model-only rmcp dependency",
        ))
    } else {
        Ok(Report::failed(failures))
    }
}
