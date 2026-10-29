#![forbid(unsafe_code)]
//! Security review of bundle `0.25.0` (design 34 U7, ADR 0047 decision 12,
//! `story:kubernetes-namespace-workspace-and-exec-slice`).
//!
//! `xtask/src/bundle/namespace_driver.rs` says it holds "one vector per table row answers exactly
//! that row", and `compatibility.json` says "a host daemon ... its answers do not change". These
//! cases render a mutant of `xtask/bundle-source/0.25.0/` into a scratch tree under
//! `CARGO_TARGET_TMPDIR` and ask `check-bundle 0.25.0` whether it refuses a source that contradicts
//! those statements. Nothing under `contracts/` or `xtask/bundle-source/` is written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const VERSION: &str = "0.25.0";
const PREDECESSOR: &str = "0.24.0";
const SIGNAL: &str =
    "documents/vectors/http/exec-signal-outside-the-published-set-is-unserved.json";
const ADOPT: &str = "documents/vectors/http/workspace-adopt-unserved-by-the-driver.json";
const BUNDLE: &str = "documents/bundle.json";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("scratch directory");
    for entry in fs::read_dir(from).expect("readable tree") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// Renders the mutated source and runs `check-bundle` on it. `extra` names further released
/// bundles the scratch contracts root needs beside the predecessor.
fn check_bundle_after(
    document: &str,
    extra: &[&str],
    mutate: impl FnOnce(&mut Value),
) -> (bool, String) {
    let scratch = tempfile::Builder::new()
        .prefix("nsdrv-sec-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("scratch tree");
    let source = scratch.path().join("source");
    let contracts = scratch.path().join("contracts");
    copy_tree(
        &repo_root().join("xtask/bundle-source").join(VERSION),
        &source.join(VERSION),
    );
    for version in std::iter::once(&PREDECESSOR).chain(extra) {
        copy_tree(
            &repo_root().join("contracts/substrate-wire").join(version),
            &contracts.join(version),
        );
    }

    let authored = source.join(VERSION).join(document);
    let mut value = read_json(&authored);
    mutate(&mut value);
    fs::write(
        &authored,
        serde_json::to_vec_pretty(&value).expect("serialize mutant"),
    )
    .expect("write mutant");

    let rendered = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["render-bundle", VERSION])
        .arg("--source")
        .arg(&source)
        .arg("--contracts-root")
        .arg(&contracts)
        .arg("--out")
        .arg(contracts.join(VERSION))
        .arg("--force")
        .output()
        .expect("run render-bundle");
    assert!(
        rendered.status.success(),
        "the mutant has to render:\n{}{}",
        String::from_utf8_lossy(&rendered.stdout),
        String::from_utf8_lossy(&rendered.stderr)
    );
    let checked = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check-bundle", VERSION])
        .arg("--source")
        .arg(&source)
        .arg("--contracts-root")
        .arg(&contracts)
        .output()
        .expect("run check-bundle");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    (checked.status.success(), said)
}

fn machine_states(vector: &mut Value) -> Vec<&mut Value> {
    vector["setup"]
        .as_array_mut()
        .expect("setup")
        .iter_mut()
        .filter(|step| step["kind"] == "machine")
        .map(|step| &mut step["state"])
        .collect()
}

#[test]
fn security_review_control_the_unmutated_scratch_bundle_checks_green() {
    let (passed, said) = check_bundle_after(ADOPT, &[], |_| {});
    assert!(
        passed,
        "the unmutated scratch bundle must check green:\n{said}"
    );
}

/// `exec.signal-unserved` answers "a signal outside the driver's published `exec.signals`"
/// (`DRIVER_UNSERVED_REFUSALS`, compatibility.json). A vector whose machine publishes `INT` and
/// still answers `INT` with the refusal states the opposite of that row.
#[test]
fn security_review_a_signal_refusal_for_a_published_signal_is_refused() {
    let (passed, said) = check_bundle_after(SIGNAL, &[], |vector| {
        let signal = vector["action"]["request"]["body"]["input"]["signal"].clone();
        assert_eq!(signal, json!("INT"), "the vector's own request");
        let states = machine_states(vector);
        assert!(!states.is_empty(), "the vector sets up a machine");
        for state in states {
            state["facts"]["exec.signals"]
                .as_array_mut()
                .expect("exec.signals")
                .insert(0, signal.clone());
        }
    });
    assert!(
        !passed,
        "check-bundle admits exec.signal-unserved for a signal the machine publishes:\n{said}"
    );
}

/// "The host driver publishes none of them and its answers do not change" (AGENTS.md invariant 6,
/// compatibility.json). A vector whose machine is `driver: host` answering
/// `workspace.adopt-unserved` contradicts it: the host serves adoption.
#[test]
fn security_review_a_host_machine_answering_a_driver_unserved_refusal_is_refused() {
    let (passed, said) = check_bundle_after(ADOPT, &[], |vector| {
        let states = machine_states(vector);
        assert!(!states.is_empty(), "the vector sets up a machine");
        for state in states {
            state["driver"] = json!("host");
        }
    });
    assert!(
        !passed,
        "check-bundle admits a host machine answering workspace.adopt-unserved:\n{said}"
    );
}

/// Design 34 section 11: `bundle.json` states predecessor `0.24.0`. The route counts are recomputed
/// against whichever predecessor the block names, so a block naming `0.23.0` recomputes to 44
/// preserved and 1 added. `schemas/bundle.json` pins all four members as constants, which is what
/// refuses it; this case keeps that pin measured.
#[test]
fn security_review_a_successor_naming_a_non_adjacent_predecessor_is_refused() {
    let (passed, said) = check_bundle_after(BUNDLE, &["0.23.0"], |bundle| {
        assert_eq!(bundle["compatibility"]["predecessor"], json!(PREDECESSOR));
        bundle["compatibility"]["predecessor"] = json!("0.23.0");
    });
    assert!(
        !passed && said.contains("bundle.json"),
        "check-bundle admits 0.25.0 naming the non-adjacent predecessor 0.23.0:\n{said}"
    );
}
