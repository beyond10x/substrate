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

/// The workspace manifest, its lock file, its vendored `[patch]` sources (linked), and every
/// member's manifest and README, copied, with an
/// empty file where each member has a library or binary root: Cargo resolves the copy as it
/// resolves the repository (the fence reads the resolved graph) and reads no source.
fn copy_manifests() -> tempfile::TempDir {
    let scratch = tempfile::tempdir().expect("scratch workspace");
    let root_text = std::fs::read_to_string(repo().join("Cargo.toml")).expect("root manifest");
    std::fs::write(scratch.path().join("Cargo.toml"), &root_text).expect("root manifest");
    std::fs::copy(repo().join("Cargo.lock"), scratch.path().join("Cargo.lock")).expect("lock file");
    std::os::unix::fs::symlink(repo().join("vendor"), scratch.path().join("vendor"))
        .expect("the vendored patches");
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
        let bins = std::fs::read_dir(repo().join(member).join("src/bin"))
            .into_iter()
            .flatten()
            .map(|entry| format!("src/bin/{}", entry.expect("a bin").file_name().display()));
        for target in ["src/lib.rs".to_owned(), "src/main.rs".to_owned()]
            .into_iter()
            .chain(bins)
        {
            if repo().join(member).join(&target).is_file() {
                let to = to.join(&target);
                std::fs::create_dir_all(to.parent().expect("a directory")).expect("member src");
                std::fs::write(to, "").expect("member target");
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

/// A package at `directory` with `manifest` and an empty library root: Cargo refuses a package
/// with no target, so a shim without one would be refused by Cargo, not by the fence.
fn write_shim(directory: &Path, manifest: &str) {
    std::fs::create_dir_all(directory.join("src")).expect("shim directory");
    std::fs::write(directory.join("Cargo.toml"), manifest).expect("shim manifest");
    std::fs::write(directory.join("src/lib.rs"), "").expect("shim library");
}

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
    write_shim(&scratch.path().join("shim"), SHIM);
    add_host_dependency(scratch.path(), "shim = { path = \"../../shim\" }");
    assert_refused(
        scratch.path(),
        "crates/substrate-host -> shim (an unlisted path member) -> fake",
    );
}

/// A path dependency outside the workspace directory is not a member at all. The fence never reads
/// its manifest, so its runtime edge to the fake passes unexamined.
///
/// The shim names the copy's client, which is the repository's own client in the copy. Naming the
/// checkout's client instead puts a second package of that name and version in the graph, which
/// Cargo refuses to lock ("package collision in the lockfile"): Cargo, not the fence, refused it.
#[test]
fn a_fake_edge_in_a_path_manifest_outside_the_root_is_refused() {
    let scratch = copy_manifests();
    let outside = tempfile::tempdir().expect("outside directory");
    write_shim(
        outside.path(),
        &SHIM.replace(
            "../crates/substrate-kubernetes-client",
            &scratch
                .path()
                .join("crates/substrate-kubernetes-client")
                .display()
                .to_string(),
        ),
    );
    add_host_dependency(
        scratch.path(),
        &format!("shim = {{ path = \"{}\" }}", outside.path().display()),
    );
    assert_refused(
        scratch.path(),
        "crates/substrate-host -> shim (a path manifest outside the root) -> fake",
    );
}

/// Why a `fake` dev-dependency edge in a member with a binary target is refused.
const BINARY_FENCE: &str = "the fake feature of b10x-substrate-kubernetes-client is refused on every edge of a member with a binary target";

/// The client as a dev-dependency with `fake`, for the member at `relative`.
fn add_fake_dev_dependency(root: &Path, relative: &str) {
    edit(root, relative, |text| {
        text.replacen(
            "[dev-dependencies]\n",
            "[dev-dependencies]\nsubstrate-kubernetes-client = { package = \"b10x-substrate-kubernetes-client\", path = \"../substrate-kubernetes-client\", version = \"=0.15.0\", features = [\"fake\"] }\n",
            1,
        )
    });
}

/// `cargo test` unifies a dev-dependency's features into the member's binary, and the tests that
/// spawn `CARGO_BIN_EXE_*` run that binary: a `fake` dev edge in the daemon builds the daemon the
/// tests drive with the in-process API server in it (design 34 § 10.1, decision D4 of the review).
#[test]
fn a_fake_dev_edge_in_a_member_with_a_binary_target_is_refused() {
    let scratch = copy_manifests();
    add_fake_dev_dependency(scratch.path(), "crates/substrate-daemon/Cargo.toml");
    let output = check_packages(scratch.path());
    let text = shown(&output);
    assert!(
        !output.status.success()
            && text.contains(BINARY_FENCE)
            && text.contains("b10x-substrate-daemon"),
        "a fake dev edge in the daemon passes:\n{text}"
    );
}

/// A library member's tests may use the fake: the edge is the one § 10.1 makes legal.
#[test]
fn a_fake_dev_edge_in_a_library_member_passes() {
    let scratch = copy_manifests();
    add_fake_dev_dependency(scratch.path(), HOST);
    let output = check_packages(scratch.path());
    assert!(output.status.success(), "{}", shown(&output));
}
