//! Keep every workspace package non-publishable and verify the source-consumption boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::report::Report;

const SOURCE_PACKAGES: [(&str, &str); 11] = [
    ("crates/substrate-wire", "b10x-substrate-wire"),
    ("crates/substrate-store", "b10x-substrate-store"),
    ("crates/substrate-guarded", "b10x-substrate-guarded"),
    ("crates/substrate-executor", "b10x-substrate-executor"),
    ("crates/substrate-host", "b10x-substrate-host"),
    (
        "crates/substrate-kubernetes-client",
        "b10x-substrate-kubernetes-client",
    ),
    ("crates/substrate-kubernetes", "b10x-substrate-kubernetes"),
    ("crates/substrate-daemon", "b10x-substrate-daemon"),
    ("crates/b10x-substrate-sdk", "b10x-substrate-sdk"),
    ("crates/b10x-substrate-cli", "b10x-substrate-cli"),
    (
        "crates/b10x-substrate-conformance",
        "b10x-substrate-conformance",
    ),
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

/// [`check`] plus the rules that read the graph Cargo resolves, which need the real workspace and
/// its lock file: the executor's dependency-tree rule, and the `fake` fence over every package
/// Cargo resolves ([`check_fake_graph`]). What `cargo xtask check-packages` runs.
pub fn check_with_executor_tree(root: &Path) -> Result<Report> {
    let (summary, mut failures) = check(root)?.into_parts();
    failures.extend(check_executor_tree(root)?);
    failures.extend(check_fake_graph(root)?);
    failures.extend(check_conformance_tree(root)?);
    if failures.is_empty() {
        Ok(Report::passed(format!(
            "{summary}; the executor's normal dependency tree names none of {}; the resolved graph enables the client's fake feature on no normal or build edge and in no member with a binary target; the conformance runner's normal and dev tree names none of {}",
            EXECUTOR_REFUSED.join(", "),
            CONFORMANCE_REFUSED.join(", ")
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
/// Colour is off whatever `CARGO_TERM_COLOR` says: a coloured tree wraps its drawing characters in
/// escape codes, and no package line would then be read by name.
fn check_executor_tree(root: &Path) -> Result<Vec<String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
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

/// The clean-room runner, which certifies an endpoint from outside every Substrate process.
const CONFORMANCE: &str = "b10x-substrate-conformance";

/// The only workspace packages the runner may depend on (design 31 § 2).
const CONFORMANCE_EDGES: [&str; 2] = ["b10x-substrate-wire", "b10x-substrate-sdk"];

/// What the runner's resolved tree may not name: it links no daemon, host driver or store.
const CONFORMANCE_REFUSED: [&str; 3] = [
    "b10x-substrate-daemon",
    "b10x-substrate-host",
    "b10x-substrate-store",
];

/// Why the runner's graph is fenced.
const CONFORMANCE_FENCE: &str = "the conformance runner depends only on b10x-substrate-wire and on b10x-substrate-sdk without features, so the binary built with -p links no daemon (design 31 § 2)";

/// The manifest half of the runner's fence: in every dependency table, a workspace path
/// dependency other than [`CONFORMANCE_EDGES`] is refused, and so is a `features` entry on the SDK
/// edge, whether written on the member's edge or inherited from `[workspace.dependencies]`.
fn check_conformance_edges(
    member: &str,
    manifest: &toml::Value,
    root_manifest: &toml::Value,
    failures: &mut Vec<String>,
) {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let workspace_dependencies = root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table);
    let mut tables: Vec<(String, &toml::Value)> = KINDS
        .iter()
        .filter_map(|kind| manifest.get(*kind).map(|table| ((*kind).to_owned(), table)))
        .collect();
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for (target, table) in targets {
            tables.extend(KINDS.iter().filter_map(|kind| {
                table
                    .get(*kind)
                    .map(|table| (format!("target.{target}.{kind}"), table))
            }));
        }
    }
    for (kind, table) in tables {
        let Some(table) = table.as_table() else {
            continue;
        };
        for (alias, dependency) in table {
            let inherited = (dependency.get("workspace").and_then(toml::Value::as_bool)
                == Some(true))
            .then(|| workspace_dependencies.and_then(|dependencies| dependencies.get(alias)))
            .flatten();
            let entries: Vec<&toml::Value> = [Some(dependency), inherited]
                .into_iter()
                .flatten()
                .collect();
            let name = entries
                .iter()
                .find_map(|entry| entry.get("package").and_then(toml::Value::as_str))
                .unwrap_or(alias);
            let has_path = entries.iter().any(|entry| entry.get("path").is_some());
            if has_path && !CONFORMANCE_EDGES.contains(&name) {
                failures.push(format!(
                    "{member}/Cargo.toml: {kind}.{alias} is a workspace path dependency; {CONFORMANCE_FENCE}"
                ));
            }
            if name == "b10x-substrate-sdk"
                && entries.iter().any(|entry| entry.get("features").is_some())
            {
                failures.push(format!(
                    "{member}/Cargo.toml: {kind}.{alias} sets features on the SDK edge; {CONFORMANCE_FENCE}"
                ));
            }
        }
    }
}

/// Runs `cargo tree --locked -p b10x-substrate-conformance -e normal,dev --prefix none` in `root`,
/// which resolves features for the runner alone, and refuses each line naming a fenced package.
fn check_conformance_tree(root: &Path) -> Result<Vec<String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .args([
            "tree",
            "--locked",
            "-p",
            CONFORMANCE,
            "-e",
            "normal,dev",
            "--prefix",
            "none",
        ])
        .output()
        .context("running cargo tree for the conformance runner")?;
    if !output.status.success() {
        return Ok(vec![format!(
            "{CONFORMANCE}: cargo tree --locked failed, so the rule checked nothing: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )]);
    }
    let tree = String::from_utf8(output.stdout).context("cargo tree wrote non-UTF-8")?;
    Ok(refused_conformance_tree_lines(&tree))
}

/// The refusals for one `cargo tree --prefix none` text. A tree whose first line is not the runner
/// selected nothing, and is refused rather than read as clean.
fn refused_conformance_tree_lines(tree: &str) -> Vec<String> {
    let name_of = |line: &str| line.split_whitespace().next().map(str::to_owned);
    let mut lines = tree.lines();
    if lines.next().and_then(name_of).as_deref() != Some(CONFORMANCE) {
        return vec![format!(
            "{CONFORMANCE}: cargo tree did not start at the runner, so the rule checked nothing"
        )];
    }
    lines
        .filter(|line| {
            name_of(line).is_some_and(|name| CONFORMANCE_REFUSED.contains(&name.as_str()))
        })
        .map(|line| {
            format!(
                "{CONFORMANCE}: dependency tree names {}; {CONFORMANCE_FENCE}",
                line.trim()
            )
        })
        .collect()
}

/// The node `cargo tree -e features` prints for the client's `fake` feature.
const FAKE_NODE: &str = "b10x-substrate-kubernetes-client feature \"fake\"";

/// Why a member with a binary target may not reach the client's `fake` feature on any edge.
const BINARY_FAKE_FENCE: &str = "the fake feature of b10x-substrate-kubernetes-client is refused on every edge of a member with a binary target: cargo test unifies a dev-dependency's features into the binary its CARGO_BIN_EXE tests run (design 34 § 10.1)";

/// The arguments that print a dependency tree with feature nodes, one node per line after its
/// depth, for every target platform.
const FEATURE_TREE: [&str; 8] = [
    "--target", "all", "--prefix", "depth", "--format", "{p}", "-e", "features",
];

/// The `fake` fence asserted on its effect, over the graph Cargo resolves rather than over the
/// manifests `workspace.members` lists, so a path member no entry lists, a path manifest outside
/// the root and a `[patch]` target are read as Cargo reads them:
///
/// - with every member selected and dev-dependencies left out, no normal or build edge enables
///   `fake`;
/// - in each member with a binary target, selected alone with its dev-dependencies, no edge
///   enables `fake`.
///
/// The graph is resolved without `--locked`, so a manifest the lock file does not describe yet is
/// still read; [`check_executor_tree`] is the rule that refuses a stale lock file. The workspace
/// tree runs first and alone, so the member trees, which run side by side, find the lock file
/// already current and none of them writes it.
fn check_fake_graph(root: &Path) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    let mut workspace = vec!["tree", "--workspace", "-e", "normal,build"];
    workspace.extend(FEATURE_TREE);
    match cargo_stdout(root, &workspace)? {
        Ok(tree) if tree.contains(&format!("{KUBERNETES_CLIENT} v")) => {
            failures.extend(fake_paths(&tree).into_iter().map(|path| {
                format!("the resolved graph enables fake on a normal or build edge: {path}; {FAKE_FENCE}")
            }));
        }
        Ok(_) => failures.push(format!(
            "the resolved graph holds no {KUBERNETES_CLIENT}, so the fake fence checked nothing"
        )),
        Err(failure) => failures.push(failure),
    }
    let members = match binary_members(root)? {
        Ok(members) => members,
        Err(failure) => {
            failures.push(failure);
            return Ok(failures);
        }
    };
    let trees = std::thread::scope(|scope| {
        let running: Vec<_> = members
            .iter()
            .map(|member| {
                scope.spawn(move || {
                    let mut alone = vec!["tree", "-p", member, "-e", "normal,build,dev"];
                    alone.extend(FEATURE_TREE);
                    cargo_stdout(root, &alone)
                })
            })
            .collect();
        running
            .into_iter()
            .map(|tree| {
                tree.join()
                    .unwrap_or_else(|_| anyhow::bail!("cargo tree panicked"))
            })
            .collect::<Result<Vec<_>>>()
    })?;
    for (member, tree) in members.iter().zip(trees) {
        match tree {
            Ok(tree) if tree.starts_with(&format!("0{member} v")) => {
                failures.extend(
                    fake_paths(&tree)
                        .into_iter()
                        .map(|path| format!("{member}: {path}; {BINARY_FAKE_FENCE}")),
                );
            }
            Ok(_) => failures.push(format!(
                "{member}: cargo tree did not start at the member, so the fake fence checked nothing"
            )),
            Err(failure) => failures.push(failure),
        }
    }
    Ok(failures)
}

/// The workspace members, as Cargo discovers them, that have a binary target.
fn binary_members(root: &Path) -> Result<Result<Vec<String>, String>> {
    let text = match cargo_stdout(root, &["metadata", "--no-deps", "--format-version", "1"])? {
        Ok(text) => text,
        Err(failure) => return Ok(Err(failure)),
    };
    let metadata: serde_json::Value =
        serde_json::from_str(&text).context("cargo metadata wrote no JSON")?;
    let listed = |value: &serde_json::Value| -> Vec<serde_json::Value> {
        value.as_array().cloned().unwrap_or_default()
    };
    let members: BTreeSet<String> = listed(&metadata["workspace_members"])
        .iter()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();
    Ok(Ok(listed(&metadata["packages"])
        .iter()
        .filter(|package| {
            package["id"]
                .as_str()
                .is_some_and(|id| members.contains(id))
        })
        .filter(|package| {
            listed(&package["targets"])
                .iter()
                .any(|target| listed(&target["kind"]).iter().any(|kind| kind == "bin"))
        })
        .filter_map(|package| package["name"].as_str().map(str::to_owned))
        .collect()))
}

/// Runs `cargo <args>` in `root`: its standard output, or the refusal that says the rule checked
/// nothing because Cargo failed.
fn cargo_stdout(root: &Path, args: &[&str]) -> Result<Result<String, String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .args(args)
        .output()
        .with_context(|| format!("running cargo {}", args.join(" ")))?;
    if !output.status.success() {
        return Ok(Err(format!(
            "cargo {} failed, so the fake fence checked nothing: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(Ok(
        String::from_utf8(output.stdout).context("cargo wrote non-UTF-8")?
    ))
}

/// Each path from a root to the client's `fake` feature node in a depth-prefixed `cargo tree -e
/// features` text, as the packages on it joined by ` -> `.
fn fake_paths(tree: &str) -> BTreeSet<String> {
    let mut stack: Vec<&str> = Vec::new();
    let mut paths = BTreeSet::new();
    for line in tree.lines() {
        let node = line.trim_start_matches(|c: char| c.is_ascii_digit());
        let Ok(depth) = line[..line.len() - node.len()].parse::<usize>() else {
            continue;
        };
        stack.truncate(depth);
        if node.starts_with(FAKE_NODE) {
            let mut path: Vec<&str> = stack
                .iter()
                .filter(|node| !node.contains(" feature \""))
                .filter_map(|node| node.split_whitespace().next())
                .collect();
            path.push(FAKE_NODE);
            paths.insert(path.join(" -> "));
        }
        stack.push(node);
    }
    paths
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
                if name == CONFORMANCE {
                    check_conformance_edges(member, &manifest, &root_manifest, failures);
                }
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

    /// The fence over the resolved graph names each path to the `fake` node and nothing else: a
    /// `default` feature of the client, a deduplicated repeat and a package of a similar name are
    /// not the node.
    #[test]
    fn fake_paths_names_each_path_to_the_fake_node_and_only_those() {
        const TREE: &str = "0b10x-substrate-host v0.15.0 (/w/crates/substrate-host)\n1shim feature \"default\"\n2shim v0.15.0 (/w/shim)\n3b10x-substrate-kubernetes-client feature \"default\"\n4b10x-substrate-kubernetes-client v0.15.0 (/w/crates/substrate-kubernetes-client)\n3b10x-substrate-kubernetes-client feature \"fake\"\n4b10x-substrate-kubernetes-client v0.15.0 (/w/crates/substrate-kubernetes-client) (*)\n1serde feature \"default\"\n2serde v1.0.229\n0b10x-substrate-daemon v0.15.0 (/w/crates/substrate-daemon)\n1b10x-substrate-kubernetes-client feature \"fake\" (*)\n1b10x-substrate-kubernetes-client-extra feature \"fake\"\n";
        assert_eq!(
            super::fake_paths(TREE).into_iter().collect::<Vec<_>>(),
            [
                "b10x-substrate-daemon -> b10x-substrate-kubernetes-client feature \"fake\"",
                "b10x-substrate-host -> shim -> b10x-substrate-kubernetes-client feature \"fake\"",
            ]
        );
        assert!(super::fake_paths(&TREE.replace("\"fake\"", "\"default\"")).is_empty());
    }

    /// Design 31 § 2: the runner's workspace edges are the wire crate and the SDK without
    /// features, in every dependency table, written on the edge or inherited from the workspace.
    #[test]
    fn the_conformance_runner_takes_no_other_workspace_edge_and_no_sdk_feature() {
        const MEMBER: &str = "crates/b10x-substrate-conformance";
        const FENCE: &str = "the conformance runner depends only on b10x-substrate-wire and on b10x-substrate-sdk without features, so the binary built with -p links no daemon (design 31 § 2)";
        const SDK: &str =
            "b10x-substrate-sdk = { path = \"../b10x-substrate-sdk\", version = \"=0.4.2\" }";
        const WIRE: &str = "substrate-wire = { package = \"b10x-substrate-wire\", path = \"../substrate-wire\", version = \"=0.4.2\" }";

        let directory = tempfile::tempdir().expect("temporary workspace");
        fixture(
            directory.path(),
            MEMBER,
            &format!("\n[dependencies]\n{SDK}\n{WIRE}\nserde = \"1\"\n"),
        );
        assert_eq!(
            check(directory.path())
                .expect("package check")
                .failure_text(),
            ""
        );

        let cases = [
            (
                format!(
                    "\n[dependencies]\n{SDK}\nhost = {{ package = \"b10x-substrate-host\", path = \"../substrate-host\", version = \"=0.4.2\" }}\n"
                ),
                format!(
                    "{MEMBER}/Cargo.toml: dependencies.host is a workspace path dependency; {FENCE}"
                ),
            ),
            (
                format!(
                    "\n[dev-dependencies]\nb10x-substrate-daemon = {{ path = \"../substrate-daemon\", version = \"=0.4.2\" }}\n\n[dependencies]\n{SDK}\n"
                ),
                format!(
                    "{MEMBER}/Cargo.toml: dev-dependencies.b10x-substrate-daemon is a workspace path dependency; {FENCE}"
                ),
            ),
            (
                "\n[target.'cfg(unix)'.build-dependencies]\nshim = { path = \"../../shim\" }\n"
                    .to_owned(),
                format!(
                    "{MEMBER}/Cargo.toml: target.cfg(unix).build-dependencies.shim is a workspace path dependency; {FENCE}"
                ),
            ),
            (
                format!(
                    "\n[dependencies]\n{}\n",
                    SDK.replace(" }", ", features = [\"linked-daemon\"] }")
                ),
                format!(
                    "{MEMBER}/Cargo.toml: dependencies.b10x-substrate-sdk sets features on the SDK edge; {FENCE}"
                ),
            ),
        ];
        for (extra, refused) in cases {
            let directory = tempfile::tempdir().expect("temporary workspace");
            fixture(directory.path(), MEMBER, &extra);
            let report = check(directory.path()).expect("package check");
            assert_eq!(report.failure_text(), refused, "{extra}");
        }

        // An edge inherited from `[workspace.dependencies]` carries the workspace entry's path and
        // features. An inherited SDK edge is also unpinned in the member, which the version rule
        // already refuses; the fence still names the feature.
        for (workspace_entry, refused) in [
            (
                "store = { package = \"b10x-substrate-store\", path = \"crates/substrate-store\" }",
                format!(
                    "{MEMBER}/Cargo.toml: dependencies.store is a workspace path dependency; {FENCE}"
                ),
            ),
            (
                "b10x-substrate-sdk = { path = \"crates/b10x-substrate-sdk\", version = \"=0.4.2\", features = [\"linked-daemon\"] }",
                format!(
                    "{MEMBER}/Cargo.toml: dependency b10x-substrate-sdk must pin version =0.4.2\n{MEMBER}/Cargo.toml: dependencies.b10x-substrate-sdk sets features on the SDK edge; {FENCE}"
                ),
            ),
        ] {
            let alias = workspace_entry.split_whitespace().next().expect("alias");
            let directory = tempfile::tempdir().expect("temporary workspace");
            fixture(
                directory.path(),
                MEMBER,
                &format!("\n[dependencies]\n{alias}.workspace = true\n"),
            );
            let root = directory.path().join("Cargo.toml");
            let mut manifest = std::fs::read_to_string(&root).expect("workspace manifest");
            write!(manifest, "\n[workspace.dependencies]\n{workspace_entry}\n")
                .expect("workspace manifest");
            std::fs::write(&root, manifest).expect("workspace manifest");
            let report = check(directory.path()).expect("package check");
            assert_eq!(report.failure_text(), refused, "{workspace_entry}");
        }
    }

    /// The tree half of the runner's fence reads `cargo tree --prefix none` output: it refuses each
    /// line naming the daemon, the host driver or the store, and nothing of a similar name.
    #[test]
    fn the_conformance_tree_refuses_the_daemon_host_and_store_and_only_those() {
        const CLEAN: &str = "b10x-substrate-conformance v0.4.2 (/w/crates/b10x-substrate-conformance)\nb10x-substrate-sdk v0.4.2 (/w/crates/b10x-substrate-sdk)\nb10x-substrate-wire v0.4.2 (/w/crates/substrate-wire)\nb10x-substrate-wire v0.4.2 (/w/crates/substrate-wire) (*)\nrustls v0.23.31\n";
        assert_eq!(
            super::refused_conformance_tree_lines(CLEAN),
            Vec::<String>::new()
        );
        for mutant in [
            "b10x-substrate-daemon v0.4.2 (/w/crates/substrate-daemon)",
            "b10x-substrate-host v0.4.2 (/w/crates/substrate-host)",
            "b10x-substrate-store v0.4.2 (/w/crates/substrate-store) (*)",
        ] {
            let refused = super::refused_conformance_tree_lines(&format!("{CLEAN}{mutant}\n"));
            assert_eq!(refused.len(), 1, "{mutant}");
            assert!(
                refused[0].starts_with(&format!(
                    "b10x-substrate-conformance: dependency tree names {mutant}; "
                )),
                "{refused:?}"
            );
        }
        for innocent in [
            "b10x-substrate-hosted v0.1.0",
            "b10x-substrate-storex v0.1.0",
        ] {
            assert_eq!(
                super::refused_conformance_tree_lines(&format!("{CLEAN}{innocent}\n")),
                Vec::<String>::new(),
                "{innocent}"
            );
        }
        for selected_nothing in ["", "b10x-substrate-sdk v0.4.2 (/w)\n"] {
            assert_eq!(
                super::refused_conformance_tree_lines(selected_nothing),
                vec![
                    "b10x-substrate-conformance: cargo tree did not start at the runner, so the rule checked nothing"
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
