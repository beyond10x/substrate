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
    if failures.is_empty() {
        Ok(Report::passed(
            "Cargo.lock has no known RustSec vulnerabilities and contains no h2",
        ))
    } else {
        Ok(Report::failed(failures))
    }
}
