#![forbid(unsafe_code)]
//! Security review pass 1 of design 34 U4: a manifest route by which a fenced crate reaches the
//! executor and `cargo xtask check-packages` accepts it.
//!
//! The executor's tree rule (`xtask/src/packages.rs`, `check_executor_tree`) runs
//! `cargo tree --locked -p b10x-substrate-executor -e normal` with no `--target`, so cargo shows
//! the dependencies of the host target only (`x86_64-unknown-linux-gnu` on every builder). The
//! executor is never built for the host target in production: the `executor-builder` stage of the
//! `Dockerfile` builds it with `--target x86_64-unknown-linux-musl`. A dependency declared under
//! `[target.'cfg(target_env = "musl")'.dependencies]` (or a musl-only edge in any crate below the
//! executor) is linked into the shipped binary and is absent from the tree the rule reads.
//!
//! The case copies the workspace's manifests, sources and lock file into a scratch tree, gives
//! the executor a musl-only `rustls` edge, lets cargo record that one edge in the copied lock
//! file offline, and asserts `check-packages` refuses it. A control asserts the unmutated copy
//! passes, so a refusal is the mutation's and not the fixture's.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXECUTOR: &str = "crates/substrate-executor/Cargo.toml";
const MUSL_ONLY_RUSTLS: &str =
    "\n[target.'cfg(target_env = \"musl\")'.dependencies]\nrustls.workspace = true\n";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("directory");
    for entry in std::fs::read_dir(from).expect("read directory") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        let kind = entry.file_type().expect("file type");
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target).expect("file");
        }
    }
}

/// The workspace manifest, its lock file, the patched `vendor/` crates, and every member's
/// manifest, README and `src/`.
fn copy_workspace() -> tempfile::TempDir {
    let scratch = tempfile::tempdir().expect("scratch workspace");
    for file in ["Cargo.toml", "Cargo.lock"] {
        std::fs::copy(repo().join(file), scratch.path().join(file)).expect("workspace file");
    }
    copy_tree(&repo().join("vendor"), &scratch.path().join("vendor"));
    let root: toml::Value =
        toml::from_str(&std::fs::read_to_string(repo().join("Cargo.toml")).expect("manifest"))
            .expect("root manifest parses");
    for member in root["workspace"]["members"]
        .as_array()
        .expect("members")
        .iter()
        .filter_map(toml::Value::as_str)
    {
        let to = scratch.path().join(member);
        std::fs::create_dir_all(&to).expect("member directory");
        for file in ["Cargo.toml", "README.md"] {
            let from = repo().join(member).join(file);
            if from.is_file() {
                std::fs::copy(&from, to.join(file)).expect("member file");
            }
        }
        copy_tree(&repo().join(member).join("src"), &to.join("src"));
    }
    scratch
}

/// Runs `check-packages` as CI does: `gate.yml` sets `CARGO_TERM_COLOR=always`, under which
/// `cargo tree` wraps its drawing characters in colour codes.
fn check_packages(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("check-packages")
        .current_dir(root)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TERM_COLOR", "always")
        .output()
        .expect("xtask runs")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_musl_only_edge_to_a_fenced_crate_is_refused_by_the_executor_tree_rule() {
    let control = copy_workspace();
    let passed = check_packages(control.path());
    assert!(
        passed.status.success(),
        "the unmutated copy passes check-packages:\n{}",
        text(&passed)
    );

    let mutant = copy_workspace();
    let manifest = mutant.path().join(EXECUTOR);
    let mut executor = std::fs::read_to_string(&manifest).expect("executor manifest");
    executor.push_str(MUSL_ONLY_RUSTLS);
    std::fs::write(&manifest, executor).expect("executor manifest");
    let locked = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(mutant.path())
        .output()
        .expect("cargo metadata runs");
    assert!(
        locked.status.success(),
        "cargo records the musl-only edge in the copied lock file:\n{}",
        text(&locked)
    );
    let musl_tree = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "b10x-substrate-executor",
            "-e",
            "normal",
            "--target",
            "x86_64-unknown-linux-musl",
        ])
        .current_dir(mutant.path())
        .output()
        .expect("cargo tree runs");
    assert!(
        String::from_utf8_lossy(&musl_tree.stdout).contains("rustls v"),
        "the shipped musl build links rustls:\n{}",
        text(&musl_tree)
    );

    let checked = check_packages(mutant.path());
    assert!(
        !checked.status.success(),
        "design 34 § 2: the executor's normal dependency tree names no rustls package, yet \
         check-packages accepted a musl-only rustls edge that the Dockerfile's musl build links:\n{}",
        text(&checked)
    );
    assert!(
        text(&checked).contains("normal dependency tree names rustls"),
        "{}",
        text(&checked)
    );
}
