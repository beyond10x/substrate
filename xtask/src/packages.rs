//! Keep every workspace package non-publishable and verify the source-consumption boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::report::Report;

const SOURCE_PACKAGES: [(&str, &str); 5] = [
    ("crates/substrate-wire", "b10x-substrate-wire"),
    ("crates/substrate-store", "b10x-substrate-store"),
    ("crates/substrate-host", "b10x-substrate-host"),
    ("crates/substrate-daemon", "b10x-substrate-daemon"),
    ("crates/b10x-substrate-sdk", "b10x-substrate-sdk"),
];

pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    let packages = check_manifests(root, &mut failures)?;

    if failures.is_empty() {
        Ok(Report::passed(format!(
            "{} source-distributed runtime packages are non-publishable, version-locked, and carry SPDX metadata and README",
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
    let approved: BTreeMap<&str, &str> = SOURCE_PACKAGES.into_iter().collect();
    let approved_names: BTreeSet<&str> = SOURCE_PACKAGES.iter().map(|(_, name)| *name).collect();
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
                        "{member}/Cargo.toml: source package must be named {expected_name}, not {name}"
                    ));
                }
                if publish != Some(false) {
                    failures.push(format!(
                        "{member}/Cargo.toml: source-distributed package must set publish = false"
                    ));
                }
                if package.get("readme").and_then(toml::Value::as_str) != Some("README.md") {
                    failures.push(format!(
                        "{member}/Cargo.toml: source-distributed package must set readme = \"README.md\""
                    ));
                } else if !root.join(member).join("README.md").is_file() {
                    failures.push(format!("{member}/README.md: source README is absent"));
                }
                if package
                    .get("license")
                    .and_then(|value| value.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    != Some(true)
                {
                    failures.push(format!(
                        "{member}/Cargo.toml: source-distributed package must inherit the workspace SPDX licence"
                    ));
                }
                if package
                    .get("documentation")
                    .and_then(toml::Value::as_str)
                    .is_none_or(|url| !url.starts_with("https://beyond10x.github.io/substrate/"))
                {
                    failures.push(format!(
                        "{member}/Cargo.toml: source-distributed package must link the public Substrate documentation"
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
    for (member, name) in SOURCE_PACKAGES {
        if observed.get(member).map(String::as_str) != Some(name) {
            failures.push(format!(
                "{member}: source-distributed runtime package is absent"
            ));
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
    use std::fmt::Write as _;

    use super::{SOURCE_PACKAGES, check};

    #[test]
    fn source_package_names_are_unique_and_prefixed() {
        let mut names = SOURCE_PACKAGES
            .iter()
            .map(|(_, name)| *name)
            .collect::<Vec<_>>();
        assert!(names.iter().all(|name| name.starts_with("b10x-substrate-")));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SOURCE_PACKAGES.len());
    }

    #[test]
    fn a_publishable_workspace_member_is_refused() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let members = SOURCE_PACKAGES
            .iter()
            .map(|(member, _)| format!("\"{member}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                "[workspace]\nmembers = [{members}]\n\n[workspace.package]\nversion = \"0.4.2\"\n"
            ),
        )
        .expect("workspace manifest");

        for (member, name) in SOURCE_PACKAGES {
            let package_directory = directory.path().join(member);
            std::fs::create_dir_all(&package_directory).expect("package directory");
            std::fs::write(package_directory.join("README.md"), format!("# {name}\n"))
                .expect("package README");
            let mut manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.4.2\"\nreadme = \"README.md\"\nlicense.workspace = true\ndocumentation = \"https://beyond10x.github.io/substrate/\"\n"
            );
            if member == "crates/substrate-host" {
                writeln!(manifest, "publish = true").expect("manifest text");
            } else {
                writeln!(manifest, "publish = false").expect("manifest text");
            }
            std::fs::write(package_directory.join("Cargo.toml"), manifest)
                .expect("package manifest");
        }

        let report = check(directory.path()).expect("package check");
        assert_eq!(
            report.failure_text(),
            "crates/substrate-host/Cargo.toml: source-distributed package must set publish = false"
        );
    }
}
