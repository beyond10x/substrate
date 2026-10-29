//! Keep every workspace package non-publishable and verify the source-consumption boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::report::Report;

const SOURCE_PACKAGES: [(&str, &str); 9] = [
    ("crates/substrate-wire", "b10x-substrate-wire"),
    ("crates/substrate-store", "b10x-substrate-store"),
    ("crates/substrate-guarded", "b10x-substrate-guarded"),
    ("crates/substrate-executor", "b10x-substrate-executor"),
    ("crates/substrate-host", "b10x-substrate-host"),
    (
        "crates/substrate-kubernetes-client",
        "b10x-substrate-kubernetes-client",
    ),
    ("crates/substrate-daemon", "b10x-substrate-daemon"),
    ("crates/b10x-substrate-sdk", "b10x-substrate-sdk"),
    ("crates/b10x-substrate-cli", "b10x-substrate-cli"),
];

pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    let packages = check_manifests(root, &mut failures)?;
    let manifests = check_boundary_invariants(root, &mut failures)?;

    if failures.is_empty() {
        Ok(Report::passed(format!(
            "{} source-distributed runtime packages are non-publishable, version-locked, and carry SPDX metadata and README; {manifests} manifests declare no Flux dependency, no beyond10x Git dependency and no non-dev edge enabling the Kubernetes client's fake feature",
            packages.len()
        )))
    } else {
        Ok(Report::failed(failures))
    }
}

/// [`check`] plus the executor's dependency-tree rule, which needs the real workspace and its
/// lock file: what `cargo xtask check-packages` runs.
pub fn check_with_executor_tree(root: &Path) -> Result<Report> {
    let (summary, mut failures) = check(root)?.into_parts();
    failures.extend(check_executor_tree(root)?);
    if failures.is_empty() {
        Ok(Report::passed(format!(
            "{summary}; the executor's normal dependency tree names none of {}",
            EXECUTOR_REFUSED.join(", ")
        )))
    } else {
        Ok(Report::failed(failures))
    }
}

/// The package whose normal dependency tree [`EXECUTOR_REFUSED`] fences.
const EXECUTOR: &str = "b10x-substrate-executor";

/// What the executor's normal dependency tree may not name (design 34 § 2): the host driver, the
/// store, the daemon and the Kubernetes client by package, and any `rustls` or `gix` package by a
/// whole name segment. The executor runs inside an image the operator chooses.
const EXECUTOR_REFUSED: [&str; 6] = [
    "b10x-substrate-host",
    "b10x-substrate-store",
    "b10x-substrate-daemon",
    "b10x-substrate-kubernetes-client",
    "rustls",
    "gix",
];

/// Runs `cargo tree --locked -p b10x-substrate-executor -e normal --target all` in `root` and
/// refuses each line that names a fenced package. Every target, not the host's alone: the executor
/// ships as an `x86_64-unknown-linux-musl` build, so a musl-only edge is as linked as any other.
fn check_executor_tree(root: &Path) -> Result<Vec<String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
        .current_dir(root)
        .args([
            "tree", "--locked", "-p", EXECUTOR, "-e", "normal", "--target", "all",
        ])
        .output()
        .context("running cargo tree for the executor")?;
    if !output.status.success() {
        return Ok(vec![format!(
            "{EXECUTOR}: cargo tree --locked failed, so the rule checked nothing: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )]);
    }
    let tree = String::from_utf8(output.stdout).context("cargo tree wrote non-UTF-8")?;
    Ok(refused_executor_tree_lines(&tree))
}

/// The characters `cargo tree` indents a package line with.
const TREE_DRAWING: [char; 5] = ['│', '├', '└', '─', ' '];

/// The refusals for one `cargo tree` text. A tree whose first line is not the executor selected
/// nothing, and is refused rather than read as clean.
fn refused_executor_tree_lines(tree: &str) -> Vec<String> {
    let names_of = |line: &str| -> Option<String> {
        line.trim_start_matches(TREE_DRAWING)
            .split_whitespace()
            .next()
            .map(str::to_owned)
    };
    let mut lines = tree.lines();
    let first = lines.next().and_then(names_of);
    if first.as_deref() != Some(EXECUTOR) {
        return vec![format!(
            "{EXECUTOR}: cargo tree did not start at the executor, so the rule checked nothing"
        )];
    }
    lines
        .filter(|line| {
            names_of(line).is_some_and(|name| {
                EXECUTOR_REFUSED.iter().any(|refused| {
                    if refused.starts_with("b10x-") {
                        name == *refused
                    } else {
                        name.split(['-', '_']).any(|segment| segment == *refused)
                    }
                })
            })
        })
        .map(|line| {
            format!(
                "{EXECUTOR}: normal dependency tree names {} (design 34 § 2)",
                line.trim_start_matches(TREE_DRAWING)
            )
        })
        .collect()
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

    let inherited = inherited_names(&root_manifest);
    let mut manifests = 1;
    check_dependency_tables("Cargo.toml", &root_manifest, &inherited, failures);
    check_fake_features("Cargo.toml", &root_manifest, &inherited, failures);

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
        check_dependency_tables(&relative, &manifest, &inherited, failures);
        check_fake_features(&relative, &manifest, &inherited, failures);
    }
    Ok(manifests)
}

/// The Kubernetes client whose `fake` feature compiles an in-process API server.
const KUBERNETES_CLIENT: &str = "b10x-substrate-kubernetes-client";

/// Why an edge enabling the client's `fake` feature outside `[dev-dependencies]` is refused.
const FAKE_FENCE: &str = "the fake feature of b10x-substrate-kubernetes-client is legal only under [dev-dependencies] (design 34 § 10.1)";

/// The package each `[workspace.dependencies]` alias names: its `package` key, or the alias itself.
fn inherited_names(root_manifest: &toml::Value) -> BTreeMap<String, String> {
    root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(alias, dependency)| {
                    let name = dependency
                        .get("package")
                        .and_then(toml::Value::as_str)
                        .unwrap_or(alias);
                    (alias.clone(), name.to_owned())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a dependency entry names the Kubernetes client. An entry inherited with
/// `workspace = true` carries no `package` key: its package is the one `[workspace.dependencies]`
/// names under the same alias, so a workspace entry that renames the client is still the client.
/// Any other entry names it by its `package` key, or by an alias that is the package name. The
/// workspace's conventional alias is refused whatever it resolves to.
fn names_kubernetes_client(
    alias: &str,
    dependency: &toml::Value,
    inherited: &BTreeMap<String, String>,
) -> bool {
    let inherits = dependency.get("workspace").and_then(toml::Value::as_bool) == Some(true);
    let name = if inherits {
        inherited.get(alias).map_or(alias, String::as_str)
    } else {
        dependency
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(alias)
    };
    name == KUBERNETES_CLIENT || alias == "substrate-kubernetes-client"
}

/// Whether a dependency entry turns the `fake` feature on.
fn enables_fake(dependency: &toml::Value) -> bool {
    dependency
        .get("features")
        .and_then(toml::Value::as_array)
        .is_some_and(|features| {
            features
                .iter()
                .any(|feature| feature.as_str() == Some("fake"))
        })
}

/// The `[features]` half of the fence. Another manifest may not forward `<client>/fake` or
/// `<client>?/fake` from a feature of its own, and the client may not turn `fake` on from any
/// other feature of its own (`default` included), because either reaches a runtime edge without
/// naming `fake` on it.
fn check_fake_features(
    manifest_path: &str,
    manifest: &toml::Value,
    inherited: &BTreeMap<String, String>,
    failures: &mut Vec<String>,
) {
    let Some(features) = manifest.get("features").and_then(toml::Value::as_table) else {
        return;
    };
    let is_client = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        == Some(KUBERNETES_CLIENT);
    let mut tables: Vec<&toml::Value> = ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .filter_map(|kind| manifest.get(*kind))
        .collect();
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for table in targets.values() {
            tables.extend(
                ["dependencies", "dev-dependencies", "build-dependencies"]
                    .iter()
                    .filter_map(|kind| table.get(*kind)),
            );
        }
    }
    let aliases: BTreeSet<&str> = tables
        .iter()
        .filter_map(|table| table.as_table())
        .flat_map(|table| table.iter())
        .filter(|(alias, dependency)| names_kubernetes_client(alias, dependency, inherited))
        .map(|(alias, _)| alias.as_str())
        .collect();

    for (feature, enabled) in features {
        let Some(enabled) = enabled.as_array() else {
            continue;
        };
        for entry in enabled.iter().filter_map(toml::Value::as_str) {
            let forwarded = entry
                .strip_suffix("/fake")
                .map(|alias| alias.trim_end_matches('?'))
                .is_some_and(|alias| aliases.contains(alias));
            if forwarded {
                failures.push(format!(
                    "{manifest_path}: features.{feature} enables {entry}; {FAKE_FENCE}"
                ));
            } else if is_client && feature != "fake" && entry == "fake" {
                failures.push(format!(
                    "{manifest_path}: features.{feature} enables fake; {FAKE_FENCE}"
                ));
            }
        }
    }
}

/// Every dependency table one manifest can carry, whatever its kind or target.
fn check_dependency_tables(
    manifest_path: &str,
    manifest: &toml::Value,
    inherited: &BTreeMap<String, String>,
    failures: &mut Vec<String>,
) {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

    for kind in KINDS {
        check_dependency_table(manifest_path, kind, manifest.get(kind), inherited, failures);
    }
    if let Some(workspace) = manifest.get("workspace") {
        check_dependency_table(
            manifest_path,
            "workspace.dependencies",
            workspace.get("dependencies"),
            inherited,
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
                    inherited,
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
                inherited,
                failures,
            );
        }
    }
}

fn check_dependency_table(
    manifest_path: &str,
    kind: &str,
    table: Option<&toml::Value>,
    inherited: &BTreeMap<String, String>,
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
        // A `[patch]` entry replaces a source and enables no feature, so only the dependency kinds
        // can carry the fence's subject.
        if !kind.ends_with("dev-dependencies")
            && !kind.starts_with("patch.")
            && names_kubernetes_client(alias, dependency, inherited)
            && enables_fake(dependency)
        {
            failures.push(format!(
                "{manifest_path}: {kind}.{alias} enables fake; {FAKE_FENCE}"
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

    /// Design 34 § 10.1: the client's `fake` feature compiles an in-process API server, so it is
    /// legal only on a `[dev-dependencies]` edge. Every other way of enabling it is refused: a
    /// runtime, build or target dependency edge, the workspace's dependency table every member
    /// inherits from, a `[features]` entry naming `<alias>/fake`, and a feature of the client's own
    /// that turns `fake` on for every edge.
    #[test]
    fn the_fake_feature_is_refused_on_every_edge_but_a_dev_dependency() {
        const EDGE: &str = "substrate-kubernetes-client = { package = \"b10x-substrate-kubernetes-client\", path = \"../substrate-kubernetes-client\", version = \"=0.4.2\", features = [\"fake\"] }";
        const FENCE: &str = "the fake feature of b10x-substrate-kubernetes-client is legal only under [dev-dependencies] (design 34 § 10.1)";
        let cases = [
            (
                "crates/substrate-host",
                format!("\n[dependencies]\n{EDGE}\n"),
                format!(
                    "crates/substrate-host/Cargo.toml: dependencies.substrate-kubernetes-client enables fake; {FENCE}"
                ),
            ),
            (
                "crates/substrate-daemon",
                format!("\n[build-dependencies]\n{EDGE}\n"),
                format!(
                    "crates/substrate-daemon/Cargo.toml: build-dependencies.substrate-kubernetes-client enables fake; {FENCE}"
                ),
            ),
            (
                "crates/substrate-host",
                format!("\n[target.'cfg(unix)'.dependencies]\n{EDGE}\n"),
                format!(
                    "crates/substrate-host/Cargo.toml: target.cfg(unix).dependencies.substrate-kubernetes-client enables fake; {FENCE}"
                ),
            ),
            (
                "crates/substrate-host",
                format!(
                    "\n[dependencies]\n{}\n\n[features]\ntesting = [\"substrate-kubernetes-client?/fake\"]\n",
                    EDGE.replace(", features = [\"fake\"]", "")
                ),
                format!(
                    "crates/substrate-host/Cargo.toml: features.testing enables substrate-kubernetes-client?/fake; {FENCE}"
                ),
            ),
            (
                "crates/substrate-kubernetes-client",
                "\n[features]\nfake = []\ndefault = [\"fake\"]\n".to_owned(),
                format!(
                    "crates/substrate-kubernetes-client/Cargo.toml: features.default enables fake; {FENCE}"
                ),
            ),
        ];
        for (member, extra, refused) in cases {
            let directory = tempfile::tempdir().expect("temporary workspace");
            fixture(directory.path(), member, &extra);
            let report = check(directory.path()).expect("package check");
            assert_eq!(report.failure_text(), refused, "{extra}");
        }

        let directory = tempfile::tempdir().expect("temporary workspace");
        fixture(directory.path(), "crates/substrate-host", "");
        let root = directory.path().join("Cargo.toml");
        let mut manifest = std::fs::read_to_string(&root).expect("workspace manifest");
        write!(manifest, "\n[workspace.dependencies]\n{EDGE}\n").expect("workspace manifest");
        std::fs::write(&root, manifest).expect("workspace manifest");
        let report = check(directory.path()).expect("package check");
        assert_eq!(
            report.failure_text(),
            format!(
                "Cargo.toml: workspace.dependencies.substrate-kubernetes-client enables fake; {FENCE}"
            )
        );

        let directory = tempfile::tempdir().expect("temporary workspace");
        fixture(
            directory.path(),
            "crates/substrate-host",
            &format!("\n[dev-dependencies]\n{EDGE}\n"),
        );
        let report = check(directory.path()).expect("package check");
        assert_eq!(report.failure_text(), "", "a dev-dependency edge is legal");
    }

    /// Design 34 § 2: the executor runs inside an image the operator chooses, so its normal
    /// dependency tree may name neither the host driver's stack nor a TLS or Git one.
    #[test]
    fn the_executor_tree_refuses_every_named_package_and_only_those() {
        const CLEAN: &str = "b10x-substrate-executor v0.4.2 (/w/crates/substrate-executor)\n├── b10x-substrate-guarded v0.4.2 (/w/crates/substrate-guarded)\n│   ├── b10x-substrate-wire v0.4.2 (/w/crates/substrate-wire)\n│   │   └── serde v1.0.229\n│   └── libc v0.2.180\n└── serde_json v1.0.151\n";
        assert_eq!(
            super::refused_executor_tree_lines(CLEAN),
            Vec::<String>::new()
        );

        for mutant in [
            "b10x-substrate-host v0.4.2 (/w/crates/substrate-host)",
            "b10x-substrate-store v0.4.2 (/w/crates/substrate-store)",
            "b10x-substrate-daemon v0.4.2 (/w/crates/substrate-daemon)",
            "b10x-substrate-kubernetes-client v0.4.2 (/w/crates/substrate-kubernetes-client)",
            "rustls v0.23.31",
            "rustls-pki-types v1.12.0",
            "tokio-rustls v0.26.2",
            "gix v0.87.1",
            "gix-features v0.43.1 (*)",
        ] {
            let tree = format!("{CLEAN}│   └── {mutant}\n");
            let refused = super::refused_executor_tree_lines(&tree);
            assert_eq!(refused.len(), 1, "{mutant}");
            assert!(refused[0].contains(mutant), "{refused:?}");
            assert!(
                refused[0].starts_with("b10x-substrate-executor: normal dependency tree names "),
                "{refused:?}"
            );
        }

        for innocent in [
            "gixy v0.1.0",
            "rustlsx v0.1.0",
            "b10x-substrate-hosted v0.1.0",
        ] {
            let tree = format!("{CLEAN}└── {innocent}\n");
            assert_eq!(
                super::refused_executor_tree_lines(&tree),
                Vec::<String>::new(),
                "{innocent}"
            );
        }

        for selected_nothing in ["", "b10x-substrate-guarded v0.4.2 (/w)\n"] {
            assert_eq!(
                super::refused_executor_tree_lines(selected_nothing),
                vec![
                    "b10x-substrate-executor: cargo tree did not start at the executor, so the rule checked nothing"
                        .to_owned()
                ]
            );
        }
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
