#![forbid(unsafe_code)]
//! Security review pass 1 of design 34 U1: two manifests that enable the Kubernetes client's
//! `fake` feature on a runtime edge and that `cargo xtask check-packages` accepts.
//!
//! The fence (`xtask/src/packages.rs`, `names_kubernetes_client`) recognises the client by an
//! entry's `package` key, or by an alias that is the package name or `substrate-kubernetes-client`.
//! An entry inherited with `workspace = true` carries no `package` key, so a workspace dependency
//! that renames the client (`k8s = { package = "b10x-substrate-kubernetes-client", … }`, with no
//! feature) lets a member write `k8s = { workspace = true, features = ["fake"] }` under
//! `[dependencies]`, or forward `"k8s/fake"` from a feature of its own. Cargo accepts both and
//! compiles the in-process API server into the member's runtime build.
//!
//! Each case copies the real workspace and member manifests into a scratch tree, applies one
//! mutation, and asserts `check-packages` refuses it. A control asserts the unmutated copy passes,
//! so a refusal is the mutation's and not the fixture's.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FENCE: &str =
    "the fake feature of b10x-substrate-kubernetes-client is legal only under [dev-dependencies]";
const RENAMED: &str = "k8s = { package = \"b10x-substrate-kubernetes-client\", path = \"crates/substrate-kubernetes-client\" }";
const HOST: &str = "crates/substrate-host/Cargo.toml";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf()
}

/// The workspace manifest and every member's manifest and README, copied.
fn copy_manifests() -> tempfile::TempDir {
    let scratch = tempfile::tempdir().expect("scratch workspace");
    let root_text = std::fs::read_to_string(repo().join("Cargo.toml")).expect("root manifest");
    std::fs::write(scratch.path().join("Cargo.toml"), &root_text).expect("root manifest");
    let root: toml::Value = toml::from_str(&root_text).expect("root manifest parses");
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
    }
    scratch
}

fn edit(root: &Path, relative: &str, change: impl FnOnce(String) -> String) {
    let path = root.join(relative);
    let text = std::fs::read_to_string(&path).expect("manifest");
    std::fs::write(&path, change(text)).expect("manifest");
}

/// The workspace dependency table gains the client under the alias `k8s`, without `fake`.
fn rename_in_workspace(root: &Path) {
    edit(root, "Cargo.toml", |text| {
        text.replacen(
            "[workspace.dependencies]\n",
            &format!("[workspace.dependencies]\n{RENAMED}\n"),
            1,
        )
    });
}

/// The host's own client edge, replaced with `replacement`.
fn replace_host_edge(root: &Path, replacement: &str) {
    edit(root, HOST, |text| {
        text.lines()
            .map(|line| {
                if line.starts_with("substrate-kubernetes-client =") {
                    replacement.to_owned()
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    });
}

fn check_packages(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().expect("a parent"))
        .arg("check-packages")
        .output()
        .expect("xtask runs")
}

fn shown(output: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_refused(root: &Path, what: &str) {
    let output = check_packages(root);
    let text = shown(&output);
    assert!(
        !output.status.success() && text.contains(FENCE),
        "{what} enables fake on a runtime edge and check-packages accepts it:\n{text}"
    );
}

#[test]
fn the_unmutated_manifests_pass() {
    let scratch = copy_manifests();
    let output = check_packages(scratch.path());
    assert!(output.status.success(), "{}", shown(&output));
}

#[test]
fn an_inherited_renamed_edge_that_adds_fake_is_refused() {
    let scratch = copy_manifests();
    rename_in_workspace(scratch.path());
    replace_host_edge(
        scratch.path(),
        "k8s = { workspace = true, features = [\"fake\"] }",
    );
    assert_refused(
        scratch.path(),
        "[dependencies] k8s = { workspace = true, features = [\"fake\"] }",
    );
}

#[test]
fn a_feature_forwarding_fake_through_an_inherited_renamed_edge_is_refused() {
    let scratch = copy_manifests();
    rename_in_workspace(scratch.path());
    replace_host_edge(scratch.path(), "k8s = { workspace = true }");
    edit(scratch.path(), HOST, |text| {
        text.replacen(
            "[dev-dependencies]\n",
            "[features]\ntesting = [\"k8s/fake\"]\n\n[dev-dependencies]\n",
            1,
        )
    });
    assert_refused(scratch.path(), "[features] testing = [\"k8s/fake\"]");
}

/// A manifest that enables `fake` on its own runtime edge, for the path-dependency cases below.
const SHIM: &str = "[package]\nname = \"shim\"\nversion = \"0.15.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nsubstrate-kubernetes-client = { package = \"b10x-substrate-kubernetes-client\", path = \"../crates/substrate-kubernetes-client\", version = \"=0.15.0\", features = [\"fake\"] }\n";

/// The host's runtime dependencies gain `dependency`, a path dependency written as `entry`.
fn add_host_dependency(root: &Path, entry: &str) {
    edit(root, HOST, |text| {
        text.replacen("[dependencies]\n", &format!("[dependencies]\n{entry}\n"), 1)
    });
}

/// Cargo adds a path dependency that resides in the workspace directory to the workspace, so a
/// manifest beneath the root that no `members` entry lists is still a workspace member. The fence
/// reads only the listed members, so a runtime shim that enables `fake` on its own edge is never
/// read, and the host's runtime edge to the shim reaches the fake.
#[test]
fn a_fake_edge_in_an_unlisted_path_member_is_refused() {
    let scratch = copy_manifests();
    let shim = scratch.path().join("shim");
    std::fs::create_dir_all(&shim).expect("shim directory");
    std::fs::write(shim.join("Cargo.toml"), SHIM).expect("shim manifest");
    add_host_dependency(scratch.path(), "shim = { path = \"../../shim\" }");
    assert_refused(
        scratch.path(),
        "crates/substrate-host -> shim (an unlisted path member) -> fake",
    );
}

/// A path dependency outside the workspace directory is not a member at all. The fence never reads
/// its manifest, so its runtime edge to the fake passes unexamined.
#[test]
fn a_fake_edge_in_a_path_manifest_outside_the_root_is_refused() {
    let scratch = copy_manifests();
    let outside = tempfile::tempdir().expect("outside directory");
    std::fs::write(
        outside.path().join("Cargo.toml"),
        SHIM.replace(
            "../crates/substrate-kubernetes-client",
            &repo()
                .join("crates/substrate-kubernetes-client")
                .display()
                .to_string(),
        ),
    )
    .expect("outside manifest");
    add_host_dependency(
        scratch.path(),
        &format!("shim = {{ path = \"{}\" }}", outside.path().display()),
    );
    assert_refused(
        scratch.path(),
        "crates/substrate-host -> shim (a path manifest outside the root) -> fake",
    );
}
