//! What `compatibility.kind: "additive-v1"` has to mean about the schemas, not only the routes.
//!
//! `xtask`'s bundle checker compares route inventories (`adds_routes`, `preserves_routes`) and
//! checks that a successor's own additions are present. Nothing compares a preserved operation's
//! *schemas* against its predecessor's, so the one change that cannot be additive — making a
//! request field a client never sent into a required one — passes `cargo xtask check-bundle`
//! today. Demonstrated on `0.10.0`: appending `"mode"` to
//! `schemas/inputs/pipe-session-start.json`'s `required` array, re-rendering, and running
//! `check-bundle 0.10.0` exits 0.
//!
//! Portable lane. Reads the released bundles and nothing else.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn contracts_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire")
}

fn versions(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(root)
        .expect("contract bundle root")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("bundle.json").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort_by_key(|version| {
        let mut parts = version
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    });
    found
}

fn required(path: &Path) -> Option<BTreeSet<String>> {
    let document: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(
        document
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

/// Collects every way one bundle root fails the schema half of `additive-v1`.
fn additivity_failures(root: &Path) -> Vec<String> {
    let mut failures = Vec::new();
    let found = versions(root);
    for pair in found.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        for (kind, direction) in [("inputs", "required"), ("results", "guaranteed")] {
            let directory = root.join(before).join("schemas").join(kind);
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name();
                let successor = root.join(after).join("schemas").join(kind).join(&name);
                let Some(old) = required(&entry.path()) else {
                    continue;
                };
                let Some(new) = required(&successor) else {
                    failures.push(format!(
                        "{after}/schemas/{kind}/{}: preserved at {before} and unreadable here",
                        name.to_string_lossy()
                    ));
                    continue;
                };
                // A request field a predecessor's client never had to send cannot become required:
                // that client's unchanged, previously valid body stops validating.
                let added: Vec<&String> = new.difference(&old).collect();
                // A result field a predecessor guaranteed cannot stop being guaranteed: that
                // client reads it unconditionally.
                let dropped: Vec<&String> = old.difference(&new).collect();
                let offending = if kind == "inputs" { added } else { dropped };
                if !offending.is_empty() {
                    failures.push(format!(
                        "{before} -> {after} schemas/{kind}/{}: {direction} set changed by \
                         {offending:?}, which no additive-v1 successor may do",
                        name.to_string_lossy()
                    ));
                }
            }
        }
    }
    failures
}

/// Every released bundle pair, and — when `SUBSTRATE_ADVERSARY_CONTRACTS_ROOT` names one — a second
/// root as well, so the same rule can be pointed at a candidate tree before it is released.
#[test]
fn an_additive_successor_never_requires_a_field_its_predecessor_did_not() {
    let mut roots = vec![contracts_root()];
    if let Some(extra) = std::env::var_os("SUBSTRATE_ADVERSARY_CONTRACTS_ROOT") {
        roots.push(PathBuf::from(extra));
    }
    let mut failures = Vec::new();
    for root in &roots {
        failures.extend(
            additivity_failures(root)
                .into_iter()
                .map(|failure| format!("{}: {failure}", root.display())),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
