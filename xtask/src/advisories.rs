use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};
use rustsec::{Database, Lockfile};

use crate::report::Report;

pub fn check(root: &Path) -> Result<Report> {
    let lockfile = Lockfile::load(root.join("Cargo.lock")).context("loading Cargo.lock")?;
    let checkout = tempfile::tempdir().context("creating RustSec advisory checkout")?;
    let output = Command::new("git")
        .args([
            "clone",
            "--depth=1",
            "--quiet",
            "https://github.com/RustSec/advisory-db.git",
        ])
        .arg(checkout.path())
        .output()
        .context("starting git for the RustSec advisory database")?;
    if !output.status.success() {
        anyhow::bail!(
            "fetching the RustSec advisory database failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let database =
        Database::open(checkout.path()).context("opening the RustSec advisory database")?;
    let mut failures: Vec<String> = database
        .vulnerabilities(&lockfile)
        .into_iter()
        .map(|item| {
            format!(
                "{}: {} {} is vulnerable ({})",
                item.advisory.id, item.package.name, item.package.version, item.advisory.title
            )
        })
        .collect();
    if lockfile
        .packages
        .iter()
        .any(|package| package.name.as_str() == "h2")
    {
        failures.push("Cargo.lock: h2 is present in the HTTP/1-only daemon workspace".to_owned());
    }
    for manifest in workspace_manifests(root)? {
        let text = std::fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let document: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", manifest.display()))?;
        if document
            .get("package")
            .and_then(|package| package.get("publish"))
            .and_then(toml::Value::as_bool)
            != Some(false)
        {
            failures.push(format!(
                "{}: [package] publish must be false",
                manifest.strip_prefix(root).unwrap_or(&manifest).display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(Report::passed(
            "Cargo.lock has no known RustSec vulnerabilities or h2, and every crate is private",
        ))
    } else {
        Ok(Report::failed(failures))
    }
}

fn workspace_manifests(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let text =
        std::fs::read_to_string(root.join("Cargo.toml")).context("reading workspace Cargo.toml")?;
    let root_manifest: toml::Value =
        toml::from_str(&text).context("parsing workspace Cargo.toml")?;
    let members = root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .context("workspace.members is absent")?;
    members
        .iter()
        .map(|member| {
            member
                .as_str()
                .map(|member| root.join(member).join("Cargo.toml"))
                .context("workspace member is not a string")
        })
        .collect()
}
