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
    let manifests = check_boundary_invariants(root, &mut failures)?;

    if failures.is_empty() {
        Ok(Report::passed(format!(
            "{} source-distributed runtime packages are non-publishable, version-locked, and carry SPDX metadata and README; {manifests} manifests declare no Flux dependency and no beyond10x Git dependency",
            packages.len()
        )))
    } else {
        Ok(Report::failed(failures))
    }
}

/// AGENTS.md invariant 1 — **Substrate is Flux-free**: no Flux crate in any dependency kind
/// (`adr/0001-substrate-is-standalone-and-flux-free.md`) — and invariant 2 — **no
/// sibling-component implementation dependency**, which for a manifest means no dependency
/// resolved from a `beyond10x` Git repository, this one included.
///
/// Every dependency kind is read: the workspace's `[workspace.dependencies]` and `[patch.*]`
/// tables, and each member's `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` and
/// their `[target.*]` forms. Returns how many manifests were read.
fn check_boundary_invariants(root: &Path, failures: &mut Vec<String>) -> Result<usize> {
    let root_text =
        std::fs::read_to_string(root.join("Cargo.toml")).context("reading workspace Cargo.toml")?;
    let root_manifest: toml::Value =
        toml::from_str(&root_text).context("parsing workspace Cargo.toml")?;

    let mut manifests = 1;
    check_dependency_tables("Cargo.toml", &root_manifest, failures);

    let members = root_manifest
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
        .context("workspace.members is absent")?;
    for member in members {
        let member = member
            .as_str()
            .context("workspace member is not a string")?;
        let relative = format!("{member}/Cargo.toml");
        let path = root.join(member).join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let manifest: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        manifests += 1;
        check_dependency_tables(&relative, &manifest, failures);
    }
    Ok(manifests)
}

/// Every dependency table one manifest can carry, whatever its kind or target.
fn check_dependency_tables(
    manifest_path: &str,
    manifest: &toml::Value,
    failures: &mut Vec<String>,
) {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

    for kind in KINDS {
        check_dependency_table(manifest_path, kind, manifest.get(kind), failures);
    }
    if let Some(workspace) = manifest.get("workspace") {
        check_dependency_table(
            manifest_path,
            "workspace.dependencies",
            workspace.get("dependencies"),
            failures,
        );
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for (target, table) in targets {
            for kind in KINDS {
                check_dependency_table(
                    manifest_path,
                    &format!("target.{target}.{kind}"),
                    table.get(kind),
                    failures,
                );
            }
        }
    }
    // A `[patch]` entry replaces a dependency's source after resolution, so an unchecked patch
    // table is a way past both invariants.
    if let Some(registries) = manifest.get("patch").and_then(toml::Value::as_table) {
        for (registry, table) in registries {
            check_dependency_table(
                manifest_path,
                &format!("patch.{registry}"),
                Some(table),
                failures,
            );
        }
    }
}

fn check_dependency_table(
    manifest_path: &str,
    kind: &str,
    table: Option<&toml::Value>,
    failures: &mut Vec<String>,
) {
    let Some(table) = table.and_then(toml::Value::as_table) else {
        return;
    };
    for (alias, dependency) in table {
        let (name, git) = match dependency.as_table() {
            Some(entry) => (
                entry
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(alias),
                entry.get("git").and_then(toml::Value::as_str),
            ),
            None => (alias.as_str(), None),
        };
        if names_flux(name) || git.is_some_and(|url| url_names(url, "flux")) {
            failures.push(format!(
                "{manifest_path}: {kind}.{alias} depends on Flux; substrate is Flux-free (invariant 1, ADR 0001)"
            ));
        }
        if git.is_some_and(|url| url_names(url, "beyond10x")) {
            failures.push(format!(
                "{manifest_path}: {kind}.{alias} resolves from a beyond10x Git repository; substrate takes no sibling-component implementation dependency (invariant 2)"
            ));
        }
    }
}

/// `flux` as a whole name segment, so `flux`, `flux-core` and `b10x_flux` are refused while
/// `influxdb` is not.
fn names_flux(name: &str) -> bool {
    name.split(['-', '_'])
        .any(|segment| segment.eq_ignore_ascii_case("flux"))
}

/// Whether a Git URL carries `segment` as a whole host or path segment. A trailing `.git` is
/// removed first, so an owner or repository named at the end of the URL still matches.
fn url_names(url: &str, segment: &str) -> bool {
    url.trim_end_matches(".git")
        .split(['/', ':', '@', '.'])
        .any(|part| part.eq_ignore_ascii_case(segment))
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
    use std::path::Path;

    use super::{SOURCE_PACKAGES, check};

    /// A workspace this check accepts: every approved member, non-publishable, with its README and
    /// metadata. `extra` is appended to `member`'s manifest so one test violates exactly one thing.
    fn fixture(root: &Path, member: &str, extra: &str) {
        let members = SOURCE_PACKAGES
            .iter()
            .map(|(member, _)| format!("\"{member}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[workspace]\nmembers = [{members}]\n\n[workspace.package]\nversion = \"0.4.2\"\n"
            ),
        )
        .expect("workspace manifest");

        for (name_member, name) in SOURCE_PACKAGES {
            let package_directory = root.join(name_member);
            std::fs::create_dir_all(&package_directory).expect("package directory");
            std::fs::write(package_directory.join("README.md"), format!("# {name}\n"))
                .expect("package README");
            let mut manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.4.2\"\nreadme = \"README.md\"\nlicense.workspace = true\ndocumentation = \"https://beyond10x.github.io/substrate/\"\npublish = false\n"
            );
            if name_member == member {
                manifest.push_str(extra);
            }
            std::fs::write(package_directory.join("Cargo.toml"), manifest)
                .expect("package manifest");
        }
    }

    /// Invariant 1 has a gate step, not only a paragraph: a Flux crate in any dependency kind is
    /// refused by name.
    #[test]
    fn a_flux_dependency_is_refused() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        fixture(
            directory.path(),
            "crates/substrate-host",
            "\n[dependencies]\nflux-core = \"0.1\"\n",
        );

        let report = check(directory.path()).expect("package check");
        assert_eq!(
            report.failure_text(),
            "crates/substrate-host/Cargo.toml: dependencies.flux-core depends on Flux; substrate is Flux-free (invariant 1, ADR 0001)"
        );
    }

    /// A crate whose name merely contains the letters is not a Flux dependency.
    #[test]
    fn a_name_containing_flux_is_not_a_flux_dependency() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        fixture(
            directory.path(),
            "crates/substrate-host",
            "\n[dependencies]\ninfluxdb = \"0.1\"\n",
        );

        let report = check(directory.path()).expect("package check");
        assert_eq!(report.failure_text(), "");
    }

    /// Invariant 2 likewise: a sibling component pulled in by Git revision is refused, in a
    /// development dependency as much as a runtime one.
    #[test]
    fn a_beyond10x_git_dependency_is_refused() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        fixture(
            directory.path(),
            "crates/substrate-daemon",
            "\n[dev-dependencies]\nharness = { git = \"https://github.com/beyond10x/harness.git\", rev = \"4d830f2\" }\n",
        );

        let report = check(directory.path()).expect("package check");
        assert_eq!(
            report.failure_text(),
            "crates/substrate-daemon/Cargo.toml: dev-dependencies.harness resolves from a beyond10x Git repository; substrate takes no sibling-component implementation dependency (invariant 2)"
        );
    }

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
