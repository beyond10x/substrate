//! A vector may only expect an outcome the bundle it lives in can represent.
//!
//! `check-bundle` proves a bundle is a fixed point of its own source. It does not read one document
//! against another, and `schemas/vector.json` is derived from the vectors themselves — so a
//! `signal` field there is typed as a bounded string, and any word at all passes. The released
//! `exit` shape is not a bounded string: `schemas/resource.json#/$defs/exit` and
//! `substrate_wire::Signal` both admit `INT`, `TERM`, `KILL` and nothing else.
//!
//! Portable lane. Reads the released bundles and nothing else.

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
    found.sort();
    found
}

fn vectors(root: &Path, version: &str) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for layer in ["driver", "http"] {
        let directory = root.join(version).join("vectors").join(layer);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(document) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            found.push((
                format!(
                    "{version}/vectors/{layer}/{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                document,
            ));
        }
    }
    found
}

/// The signals the released `exit` shape admits, read out of the bundle rather than restated.
fn admitted_signals(root: &Path, version: &str) -> Vec<String> {
    let path = root.join(version).join("schemas/resource.json");
    let document: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("resource schema")).expect("JSON");
    document
        .pointer("/$defs/exit/properties/signal/enum")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Every `exit.signal` a vector expects is a signal the same bundle's `exit` shape admits, and one
/// `substrate_wire::ExecExit` can carry.
///
/// A vector that expects an outcome no conforming implementation can produce is not a target; it is
/// a statement to a reader that will never be true, and it is the only kind of contract defect a
/// fixed-point check is structurally unable to see.
#[test]
fn a_vector_never_expects_an_exit_signal_the_bundle_cannot_carry() {
    let root = contracts_root();
    let mut failures = Vec::new();
    for version in versions(&root) {
        let admitted = admitted_signals(&root, &version);
        assert!(
            !admitted.is_empty(),
            "{version}/schemas/resource.json: no exit signal enum to read"
        );
        for (name, vector) in vectors(&root, &version) {
            let Some(signal) = vector
                .pointer("/expected/outcome/exit/signal")
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !admitted.iter().any(|value| value == signal) {
                failures.push(format!(
                    "{name}: expects exit.signal {signal:?}, and \
                     {version}/schemas/resource.json admits only {admitted:?}"
                ));
            }
            if serde_json::from_value::<substrate_wire::Signal>(Value::String(signal.to_owned()))
                .is_err()
            {
                failures.push(format!(
                    "{name}: expects exit.signal {signal:?}, which substrate_wire::ExecExit \
                     cannot carry, so no daemon can ever report it"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
