//! Reject a Rust toolchain version that the three pinning files do not agree on.
//!
//! `rust-toolchain.toml` decides which compiler builds this repository, `Cargo.toml`
//! `rust-version` states the minimum it claims to support, and the `Dockerfile` builder tag
//! decides which compiler the image is built with. A commit that changes one and not the others
//! reintroduces the local/CI clippy drift the pin exists to remove.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::report::Report;

/// The components the gate itself runs, so the pin must declare them.
const REQUIRED_COMPONENTS: [&str; 2] = ["rustfmt", "clippy"];

/// One file's reading: the version it pins, and the `file:line: value` line that shows it.
type Reading = Option<(String, String)>;

pub fn check(root: &Path) -> Report {
    let root = root.canonicalize().unwrap_or_else(|_| PathBuf::from(root));
    let mut failures = Vec::new();
    let readings = [
        toolchain_channel(&root, &mut failures),
        cargo_rust_version(&root, &mut failures),
        dockerfile_builder(&root, &mut failures),
    ];
    if readings.iter().any(Option::is_none) {
        return Report::failed(failures);
    }

    let versions: BTreeSet<&str> = readings
        .iter()
        .flatten()
        .map(|(version, _)| version.as_str())
        .collect();
    if versions.len() > 1 {
        failures.push(
            "Rust toolchain version disagreement; a bump is one commit that changes all three:"
                .to_owned(),
        );
        failures.extend(
            readings
                .iter()
                .flatten()
                .map(|(_, shown)| format!("  {shown}")),
        );
    }
    if !failures.is_empty() {
        return Report::failed(failures);
    }

    let version = versions.iter().next().copied().unwrap_or_default();
    Report::passed(format!(
        "Rust toolchain pinned at {version} in rust-toolchain.toml, Cargo.toml, Dockerfile"
    ))
}

fn read_lines(root: &Path, name: &str, failures: &mut Vec<String>) -> Option<Vec<String>> {
    let path = root.join(name);
    let Ok(text) = std::fs::read_to_string(&path) else {
        failures.push(format!(
            "{name}: missing under {}; the pinned version cannot be checked",
            root.display()
        ));
        return None;
    };
    Some(text.lines().map(ToOwned::to_owned).collect())
}

fn toolchain_channel(root: &Path, failures: &mut Vec<String>) -> Reading {
    let lines = read_lines(root, "rust-toolchain.toml", failures)?;
    let Some((channel, number)) =
        in_table(&lines, "toolchain", |line| quoted_value(line, "channel"))
    else {
        failures.push("rust-toolchain.toml: no [toolchain] channel".to_owned());
        return None;
    };
    let components = in_table(&lines, "toolchain", list_value);
    let declared = components
        .as_ref()
        .map_or_else(BTreeSet::new, |(inner, _)| {
            inner
                .split(',')
                .filter(|value| !value.trim().is_empty())
                .map(|value| {
                    value
                        .trim()
                        .trim_matches(|character| character == '"' || character == '\'')
                        .to_owned()
                })
                .collect()
        });
    for component in REQUIRED_COMPONENTS {
        if !declared.contains(component) {
            let at = components.as_ref().map_or(number, |(_, line)| *line);
            failures.push(format!(
                "rust-toolchain.toml:{at}: components must include '{component}'; the gate runs it"
            ));
        }
    }
    Some((
        channel.clone(),
        format!("rust-toolchain.toml:{number}: channel = \"{channel}\""),
    ))
}

fn cargo_rust_version(root: &Path, failures: &mut Vec<String>) -> Reading {
    let lines = read_lines(root, "Cargo.toml", failures)?;
    let Some((version, number)) = in_table(&lines, "workspace.package", |line| {
        quoted_value(line, "rust-version")
    }) else {
        failures.push("Cargo.toml: no [workspace.package] rust-version".to_owned());
        return None;
    };
    Some((
        version.clone(),
        format!("Cargo.toml:{number}: rust-version = \"{version}\""),
    ))
}

fn dockerfile_builder(root: &Path, failures: &mut Vec<String>) -> Reading {
    let lines = read_lines(root, "Dockerfile", failures)?;
    for (index, line) in lines.iter().enumerate() {
        if let Some(version) = builder_version(line) {
            let number = index + 1;
            let shown = format!("Dockerfile:{number}: FROM rust:{version}-\u{2026}");
            return Some((version, shown));
        }
    }
    failures.push("Dockerfile: no `FROM rust:<version>-\u{2026}` builder stage".to_owned());
    None
}

/// The first value `matcher` accepts inside `[table]`, with its one-based line number.
fn in_table(
    lines: &[String],
    table: &str,
    mut matcher: impl FnMut(&str) -> Option<String>,
) -> Option<(String, usize)> {
    let mut current = "";
    for (index, line) in lines.iter().enumerate() {
        if let Some(heading) = table_heading(line) {
            current = heading;
            continue;
        }
        if current != table {
            continue;
        }
        if let Some(value) = matcher(line) {
            return Some((value, index + 1));
        }
    }
    None
}

/// `[table]` on a line of its own.
fn table_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    if inner.is_empty() || inner.contains(']') {
        return None;
    }
    Some(inner.trim())
}

/// `key = "value"`, the quote being either kind, as the predecessor's regex allowed.
fn quoted_value(line: &str, key: &str) -> Option<String> {
    let rest = after_key(line, key)?;
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(['"', '\''])?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_owned())
}

/// `components = [ ... ]`, returning the text between the brackets.
fn list_value(line: &str) -> Option<String> {
    let rest = after_key(line, "components")?.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(rest[..end].to_owned())
}

fn after_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    Some(
        line.trim_start()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim_start(),
    )
}

/// The version in a `FROM rust:<version>-<suffix>` builder stage.
fn builder_version(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("FROM")?;
    let trimmed = rest.trim_start();
    if trimmed.len() == rest.len() {
        return None;
    }
    let rest = trimmed.strip_prefix("rust:")?;
    if !rest.chars().next()?.is_ascii_digit() {
        return None;
    }
    let end = rest
        .find(|character: char| character.is_whitespace() || character == '@' || character == '-')
        .unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::check;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    const TOOLCHAIN: &str =
        "[toolchain]\nchannel = \"1.97\"\ncomponents = [\"rustfmt\", \"clippy\"]\n";
    const CARGO: &str = "[workspace]\nmembers = [\"crates/x\"]\n\n[workspace.package]\nedition = \"2024\"\nrust-version = \"1.97\"\n";
    const DOCKERFILE: &str = "# syntax=docker/dockerfile:1.7\nFROM rust:1.97-bookworm@sha256:0e2b AS builder\nWORKDIR /src\n";

    fn tree() -> TempDir {
        let directory = tempfile::tempdir().expect("temporary directory");
        write(directory.path(), "rust-toolchain.toml", TOOLCHAIN);
        write(directory.path(), "Cargo.toml", CARGO);
        write(directory.path(), "Dockerfile", DOCKERFILE);
        directory
    }

    fn write(root: &Path, name: &str, contents: &str) {
        fs::write(root.join(name), contents).expect("write fixture");
    }

    #[test]
    fn a_tree_that_agrees_names_the_version() {
        let directory = tree();
        let report = check(directory.path());
        assert_eq!(report.failures(), &[] as &[String]);
        assert_eq!(
            report.summary(),
            "Rust toolchain pinned at 1.97 in rust-toolchain.toml, Cargo.toml, Dockerfile"
        );
    }

    #[test]
    fn a_dockerfile_builder_tag_of_1_98_disagrees() {
        let directory = tree();
        write(
            directory.path(),
            "Dockerfile",
            &DOCKERFILE.replace("1.97", "1.98"),
        );
        let report = check(directory.path());
        let text = report.failure_text();
        assert!(
            text.contains(
                "Rust toolchain version disagreement; a bump is one commit that changes all three:"
            ),
            "{text}"
        );
        assert!(
            text.contains("rust-toolchain.toml:2: channel = \"1.97\""),
            "{text}"
        );
        assert!(
            text.contains("Cargo.toml:6: rust-version = \"1.97\""),
            "{text}"
        );
        assert!(text.contains("Dockerfile:2: FROM rust:1.98-…"), "{text}");
    }

    #[test]
    fn a_cargo_rust_version_of_1_98_disagrees() {
        let directory = tree();
        write(
            directory.path(),
            "Cargo.toml",
            &CARGO.replace("1.97", "1.98"),
        );
        let report = check(directory.path());
        let text = report.failure_text();
        assert!(
            text.contains("Rust toolchain version disagreement"),
            "{text}"
        );
        assert!(
            text.contains("Cargo.toml:6: rust-version = \"1.98\""),
            "{text}"
        );
        assert!(text.contains("Dockerfile:2: FROM rust:1.97-…"), "{text}");
    }

    #[test]
    fn a_missing_required_component_fails() {
        let directory = tree();
        write(
            directory.path(),
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.97\"\ncomponents = [\"clippy\"]\n",
        );
        let report = check(directory.path());
        let text = report.failure_text();
        assert_eq!(
            text,
            "rust-toolchain.toml:3: components must include 'rustfmt'; the gate runs it"
        );
    }

    #[test]
    fn declaring_no_components_at_all_fails_for_both() {
        let directory = tree();
        write(
            directory.path(),
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.97\"\n",
        );
        let report = check(directory.path());
        let text = report.failure_text();
        assert!(
            text.contains("rust-toolchain.toml:2: components must include 'rustfmt'"),
            "{text}"
        );
        assert!(
            text.contains("rust-toolchain.toml:2: components must include 'clippy'"),
            "{text}"
        );
    }

    #[test]
    fn a_missing_pinning_file_fails_before_the_versions_are_compared() {
        let directory = tree();
        fs::remove_file(directory.path().join("Dockerfile")).expect("remove");
        let report = check(directory.path());
        let text = report.failure_text();
        assert!(text.contains("Dockerfile: missing under "), "{text}");
        assert!(
            text.contains("; the pinned version cannot be checked"),
            "{text}"
        );
        assert!(!text.contains("disagreement"), "{text}");
    }

    #[test]
    fn a_channel_outside_the_toolchain_table_is_not_read() {
        let directory = tree();
        write(
            directory.path(),
            "rust-toolchain.toml",
            "[other]\nchannel = \"1.97\"\ncomponents = [\"rustfmt\", \"clippy\"]\n",
        );
        let report = check(directory.path());
        let text = report.failure_text();
        assert_eq!(text, "rust-toolchain.toml: no [toolchain] channel");
    }

    #[test]
    fn a_dockerfile_without_a_rust_builder_fails() {
        let directory = tree();
        write(directory.path(), "Dockerfile", "FROM debian:bookworm\n");
        let report = check(directory.path());
        assert_eq!(
            report.failure_text(),
            "Dockerfile: no `FROM rust:<version>-…` builder stage"
        );
    }
}
