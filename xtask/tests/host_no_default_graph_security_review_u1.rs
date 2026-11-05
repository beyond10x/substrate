#![forbid(unsafe_code)]
//! Security review pass 1 of
//! `story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes`: a manifest
//! route by which the Git stack reaches the host driver's no-default-features graph and
//! `cargo xtask check-packages` accepts it.
//!
//! The story's first acceptance line is that `cargo tree -p b10x-substrate-host
//! --no-default-features -e normal` names none of gix, git2, curl, openssl-sys or
//! b10x-substrate-kubernetes-client. `check-packages` fixes the host's own `[features]` table
//! (`check_host_features`) and nothing else: no rule reads the graph Cargo resolves for the host
//! without its features, as `check_conformance_tree` does for the conformance runner. So a hard
//! dependency of the host that gains a Git edge puts git2 and openssl-sys into every
//! confinement-only consumer's lockfile, and the gate stays green.
//!
//! The case copies the workspace's manifests, sources and lock file into a scratch tree, gives
//! `b10x-substrate-guarded` (a non-optional dependency of the host) a `git2` edge, lets cargo
//! record it in the copied lock file offline, shows the host's no-default tree now names git2
//! and openssl-sys, and asserts `check-packages` refuses it. A control asserts the unmutated copy
//! passes, so a refusal is the mutation's and not the fixture's.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GUARDED: &str = "crates/substrate-guarded/Cargo.toml";
const UNIX_GIT2: &str = "\n[target.'cfg(unix)'.dependencies]\ngit2.workspace = true\n";

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

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
}

/// Runs `check-packages` as CI does: `gate.yml` sets `CARGO_TERM_COLOR=always`.
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
fn a_git_edge_below_the_host_reaches_its_no_default_graph_and_check_packages_refuses_it() {
    let control = copy_workspace();
    let passed = check_packages(control.path());
    assert!(
        passed.status.success(),
        "the unmutated copy passes check-packages:\n{}",
        text(&passed)
    );

    let mutant = copy_workspace();
    let manifest = mutant.path().join(GUARDED);
    let mut guarded = std::fs::read_to_string(&manifest).expect("guarded manifest");
    guarded.push_str(UNIX_GIT2);
    std::fs::write(&manifest, guarded).expect("guarded manifest");
    let locked = cargo()
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(mutant.path())
        .output()
        .expect("cargo metadata runs");
    assert!(
        locked.status.success(),
        "cargo records the git2 edge in the copied lock file:\n{}",
        text(&locked)
    );
    let tree = cargo()
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "b10x-substrate-host",
            "--no-default-features",
            "-e",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(mutant.path())
        .output()
        .expect("cargo tree runs");
    let tree_text = String::from_utf8_lossy(&tree.stdout);
    assert!(
        tree.status.success()
            && tree_text.contains("git2 v")
            && tree_text.contains("openssl-sys v"),
        "the host's no-default graph links git2 and openssl-sys:\n{}",
        text(&tree)
    );

    let checked = check_packages(mutant.path());
    assert!(
        !checked.status.success(),
        "story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes: \
         `cargo tree -p b10x-substrate-host --no-default-features -e normal` must name none of \
         gix, git2, curl, openssl-sys or b10x-substrate-kubernetes-client, yet check-packages \
         accepted a git2 edge on b10x-substrate-guarded that every confinement-only consumer \
         links:\n{}",
        text(&checked)
    );
    assert!(
        text(&checked).contains("git2"),
        "the refusal names the package:\n{}",
        text(&checked)
    );
}
