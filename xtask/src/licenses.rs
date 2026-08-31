//! The public binary carries the licence texts for the exact locked graph it redistributes.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};
use tempfile::NamedTempFile;

use crate::report::Report;

const CARGO_ABOUT_VERSION: &str = "cargo-about 0.9.1";

pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    check_workspace_license(root, &mut failures)?;

    let version = Command::new("cargo-about")
        .arg("--version")
        .output()
        .context(
            "starting cargo-about; install cargo-about 0.9.1 with cargo install --locked --features cli",
        )?;
    let observed = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    if !version.status.success() || observed != CARGO_ABOUT_VERSION {
        failures.push(format!(
            "cargo-about: expected {CARGO_ABOUT_VERSION}, observed {observed:?}"
        ));
        return Ok(Report::failed(failures));
    }

    let generated = NamedTempFile::new().context("creating third-party notice output")?;
    let output = Command::new("cargo-about")
        .current_dir(root)
        .args([
            "generate",
            "--workspace",
            "--locked",
            "--fail",
            "--output-file",
        ])
        .arg(generated.path())
        .arg("about.hbs")
        .output()
        .context("generating third-party notices")?;
    if !output.status.success() {
        failures.push(format!(
            "cargo-about generate failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return Ok(Report::failed(failures));
    }

    let expected = std::fs::read(root.join("THIRD_PARTY_LICENSES.html"))
        .context("reading THIRD_PARTY_LICENSES.html")?;
    if expected.contains(&b'\r') {
        failures.push(
            "THIRD_PARTY_LICENSES.html: committed notice must use canonical LF line endings"
                .to_owned(),
        );
    }
    // cargo-about 0.9.1 writes CRLF on every platform. The repository byte form is LF so Git's
    // `core.autocrlf` cannot make a locally generated notice disagree with a clean CI checkout.
    let actual =
        String::from_utf8(std::fs::read(generated.path()).context("reading generated notices")?)
            .context("cargo-about generated non-UTF-8 notices")?
            .replace("\r\n", "\n")
            .into_bytes();
    if expected != actual {
        failures.push(
            "THIRD_PARTY_LICENSES.html: not the cargo-about 0.9.1 fixed point of Cargo.lock"
                .to_owned(),
        );
    }

    if failures.is_empty() {
        Ok(Report::passed(
            "workspace is Apache-2.0 and third-party notices match the locked dependency graph",
        ))
    } else {
        Ok(Report::failed(failures))
    }
}

fn check_workspace_license(root: &Path, failures: &mut Vec<String>) -> Result<()> {
    let workspace_text =
        std::fs::read_to_string(root.join("Cargo.toml")).context("reading Cargo.toml")?;
    let workspace: toml::Value = toml::from_str(&workspace_text).context("parsing Cargo.toml")?;
    if workspace
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("license"))
        .and_then(toml::Value::as_str)
        != Some("Apache-2.0")
    {
        failures.push("Cargo.toml: workspace.package.license must be Apache-2.0".to_owned());
    }

    let Some(members) = workspace
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
    else {
        anyhow::bail!("Cargo.toml: workspace.members is absent");
    };
    for member in members {
        let member = member
            .as_str()
            .context("workspace member is not a string")?;
        let path = root.join(member).join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let manifest: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if manifest
            .get("package")
            .and_then(|value| value.get("license"))
            .and_then(|value| value.get("workspace"))
            .and_then(toml::Value::as_bool)
            != Some(true)
        {
            failures.push(format!(
                "{member}/Cargo.toml: package.license.workspace must be true"
            ));
        }
    }
    Ok(())
}
