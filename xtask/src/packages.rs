//! Keep registry publication closed and prove that every approved archive can be assembled.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};

use crate::report::Report;

const PUBLIC_PACKAGES: [(&str, &str); 5] = [
    ("crates/substrate-wire", "b10x-substrate-wire"),
    ("crates/substrate-store", "b10x-substrate-store"),
    ("crates/substrate-host", "b10x-substrate-host"),
    ("crates/substrate-daemon", "b10x-substrate-daemon"),
    ("crates/b10x-substrate-sdk", "b10x-substrate-sdk"),
];

pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    let packages = check_manifests(root, &mut failures)?;

    for (_, name) in PUBLIC_PACKAGES {
        let output = Command::new("cargo")
            .current_dir(root)
            .args([
                "package",
                "--package",
                name,
                "--locked",
                "--allow-dirty",
                "--list",
            ])
            .output()
            .with_context(|| format!("starting cargo package for {name}"))?;
        if !output.status.success() {
            failures.push(format!(
                "{name}: cargo package --list failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            continue;
        }
        let package_list = String::from_utf8_lossy(&output.stdout);
        let files: BTreeSet<&str> = package_list.lines().collect();
        if !files.contains("README.md") {
            failures.push(format!("{name}: package archive omits README.md"));
        }
    }

    if failures.is_empty() {
        Ok(Report::passed(format!(
            "{} approved registry packages are version-locked and assemble with SPDX metadata and README",
            packages.len()
        )))
    } else {
        Ok(Report::failed(failures))
    }
}

fn check_manifests(root: &Path, failures: &mut Vec<String>) -> Result<BTreeMap<String, String>> {
    let root_text =
        std::fs::read_to_string(root.join("Cargo.toml")).context("reading workspace Cargo.toml")?;
    let root_manifest: toml::Value =
        toml::from_str(&root_text).context("parsing workspace Cargo.toml")?;
    let version = root_manifest
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("version"))
        .and_then(toml::Value::as_str)
        .context("workspace.package.version is absent")?;
    let exact_version = format!("={version}");
    let members = root_manifest
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
        .context("workspace.members is absent")?;
    let approved: BTreeMap<&str, &str> = PUBLIC_PACKAGES.into_iter().collect();
    let approved_names: BTreeSet<&str> = PUBLIC_PACKAGES.iter().map(|(_, name)| *name).collect();
    let mut observed = BTreeMap::new();

    for member in members {
        let member = member
            .as_str()
            .context("workspace member is not a string")?;
        let path = root.join(member).join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let manifest: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let package = manifest
            .get("package")
            .context("workspace member has no package table")?;
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .context("workspace package has no name")?;
        let publish = package.get("publish").and_then(toml::Value::as_bool);
        match approved.get(member) {
            Some(expected_name) => {
                if name != *expected_name {
                    failures.push(format!(
                        "{member}/Cargo.toml: approved package must be named {expected_name}, not {name}"
                    ));
                }
                if publish != Some(true) {
                    failures.push(format!(
                        "{member}/Cargo.toml: approved package must set publish = true"
                    ));
                }
                if package.get("readme").and_then(toml::Value::as_str) != Some("README.md") {
                    failures.push(format!(
                        "{member}/Cargo.toml: approved package must set readme = \"README.md\""
                    ));
                }
                if package
                    .get("license")
                    .and_then(|value| value.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    != Some(true)
                {
                    failures.push(format!(
                        "{member}/Cargo.toml: approved package must inherit the workspace SPDX licence"
                    ));
                }
                check_dependencies(member, &manifest, &approved_names, &exact_version, failures);
                observed.insert(member.to_owned(), name.to_owned());
            }
            None if publish != Some(false) => failures.push(format!(
                "{member}/Cargo.toml: unapproved workspace package must set publish = false"
            )),
            None => {}
        }
    }
    for (member, name) in PUBLIC_PACKAGES {
        if observed.get(member).map(String::as_str) != Some(name) {
            failures.push(format!("{member}: approved registry package is absent"));
        }
    }
    Ok(observed)
}

fn check_dependencies(
    member: &str,
    manifest: &toml::Value,
    approved_names: &BTreeSet<&str>,
    exact_version: &str,
    failures: &mut Vec<String>,
) {
    let Some(dependencies) = manifest.get("dependencies").and_then(toml::Value::as_table) else {
        return;
    };
    for (alias, dependency) in dependencies {
        let Some(table) = dependency.as_table() else {
            continue;
        };
        let name = table
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(alias);
        if approved_names.contains(name)
            && table.get("version").and_then(toml::Value::as_str) != Some(exact_version)
        {
            failures.push(format!(
                "{member}/Cargo.toml: dependency {alias} must pin version {exact_version}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PUBLIC_PACKAGES;

    #[test]
    fn public_package_names_are_unique_and_prefixed() {
        let mut names = PUBLIC_PACKAGES
            .iter()
            .map(|(_, name)| *name)
            .collect::<Vec<_>>();
        assert!(names.iter().all(|name| name.starts_with("b10x-substrate-")));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PUBLIC_PACKAGES.len());
    }
}
