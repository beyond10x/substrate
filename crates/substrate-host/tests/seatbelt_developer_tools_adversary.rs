// Adversary cases for `story:confined-cargo-build-links-without-the-xcrun-shim`: the macos
// adapter resolves the developer tools' SDK and `clang` at open and gives a start whose exec
// subpaths hold the whole developer root `SDKROOT` and the daemon's own
// `CARGO_TARGET_<TRIPLE>_LINKER`. Each case runs a confined build, or a confined `env`, through
// the public adapter, with no `RUSTC`, `SDKROOT`, `CC` or linker set by the caller.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use substrate_host::confinement::{
    self, AdapterRequest, ConfinementAdapter, ConfinementProfile, RunState, macos::MacosRequest,
};
use substrate_host::seatbelt;
use substrate_wire::ReadOnlyRoot;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn macos() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Macos(MacosRequest::default()))
        .expect("macos opens on a macOS host whose Seatbelt sandbox enforces")
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

struct Ended {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Ended {
    fn report(&self) -> String {
        format!(
            "exit {:?}; stdout {:?}; stderr {:?}",
            self.code, self.stdout, self.stderr
        )
    }
}

async fn run_to_end(profile: &ConfinementProfile, command: &confinement::Command) -> Ended {
    let adapter = macos();
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter.start(&admitted, command).await.expect("started");
    let ended = tokio::time::timeout(Duration::from_mins(10), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let output = adapter.output(&run).await.expect("output");
    Ended {
        code: ended.exit.and_then(|exit| exit.code),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("directory");
    for entry in fs::read_dir(from).expect("fixture directory") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

/// The toolchain `rust-toolchain.toml` selects for this crate, resolved.
fn sysroot() -> PathBuf {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rustc runs");
    assert!(output.status.success(), "rustc --print sysroot failed");
    let text = String::from_utf8(output.stdout).expect("UTF-8 sysroot");
    fs::canonicalize(text.trim()).expect("the sysroot resolves")
}

/// `cargo test --offline <filter>` of a copy of `fixture` under the build policy of
/// `seatbelt_adversary.rs`, with `developer` as the exec subpath for the developer tools and no
/// `RUSTC`, `SDKROOT`, `CC` or linker set.
async fn cargo_test(fixture: &str, developer: &Path, sealed: &[&str], filter: &str) -> Ended {
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = fs::canonicalize(root.path())
        .expect("canonical")
        .join("fixture");
    copy_tree(&Path::new(FIXTURES).join(fixture), &workspace);
    let toolchain = text(&sysroot()).to_owned();
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: toolchain.clone(),
            mount: toolchain.clone(),
        }],
        exec_subpaths: vec![
            "target".into(),
            toolchain.clone(),
            "/usr/bin".into(),
            text(developer).to_owned(),
        ],
        sealed_paths: sealed.iter().copied().map(str::to_owned).collect(),
        private_tmpdir: true,
        ..ConfinementProfile::default()
    };
    let argv = vec![
        format!("{toolchain}/bin/cargo"),
        "test".into(),
        "--offline".into(),
        filter.to_owned(),
    ];
    let env = [
        ("PATH", format!("{toolchain}/bin:/usr/bin:/bin")),
        ("HOME", text(&workspace).to_owned()),
        ("CARGO_HOME", text(&workspace.join("cargo-home")).to_owned()),
        ("CARGO_TERM_COLOR", "never".to_owned()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect();
    run_to_end(&profile, &confinement::Command { argv, env }).await
}

// A1. A crate whose build script compiles C, as every crate depending on the `cc` crate does
// (`ring`, `libsqlite3-sys`, `zstd-sys`, …), builds under the same policy the acceptance fixture
// builds under. The adapter names the linker but not `CC` or `AR`, so the build script's `cc`
// is the `/usr/bin/cc` shim, which asks `xcodebuild` through `/bin/sh` and fails.
#[tokio::test(flavor = "multi_thread")]
async fn cargo_test_of_a_fixture_whose_build_script_compiles_c_passes() {
    let developer = seatbelt::developer_root().expect("the runner has developer tools");
    let ended = cargo_test("cargo-test-c", &developer, &[], "the_c_library_answers").await;
    assert_eq!(ended.code, Some(0), "{}", ended.report());
    assert!(
        ended.stdout.contains("test result: ok. 1 passed; 0 failed"),
        "{}",
        ended.report()
    );
}

// A2. The selected developer directory, what `xcode-select -p` prints
// (`…/Xcode.app/Contents/Developer`), is an admitted exec subpath and holds both the SDK and the
// toolchain `clang` the adapter resolved. The adapter gives `SDKROOT` and the linker only to a
// policy holding the whole bundle, so this policy's build reaches the `/usr/bin/cc` shim and
// fails to link, with no named refusal.
#[tokio::test(flavor = "multi_thread")]
async fn cargo_test_with_the_selected_developer_directory_as_its_exec_subpath_passes() {
    let developer = seatbelt::developer_root().expect("the runner has developer tools");
    let selected =
        fs::canonicalize(seatbelt::XCODE_SELECT_LINK).expect("precondition: an Xcode is selected");
    assert!(
        selected != developer && selected.starts_with(&developer),
        "precondition: the selected directory {} lies inside the developer root {}",
        selected.display(),
        developer.display()
    );
    let ended = cargo_test(
        "cargo-test",
        &selected,
        &[".cargo", "vendor"],
        "the_vendored_dependency_answers",
    )
    .await;
    assert_eq!(ended.code, Some(0), "{}", ended.report());
    assert!(
        ended.stdout.contains("test result: ok. 1 passed; 0 failed"),
        "{}",
        ended.report()
    );
}

// A3. The linker variable is derived from the architecture the daemon was built for, not from the
// toolchain the start runs. A toolchain whose host is the other Apple triple (an x86_64 rustup
// toolchain on Apple silicon, or a daemon running under Rosetta) or a `--target` build for it
// reads `CARGO_TARGET_<the other triple>_LINKER`, which the start does not set, and links through
// the `/usr/bin/cc` shim. The resolved `clang` links either Apple triple.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_over_the_developer_tools_names_a_linker_for_each_apple_darwin_triple() {
    let developer = seatbelt::developer_root().expect("the runner has developer tools");
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = fs::canonicalize(root.path()).expect("canonical");
    let profile = ConfinementProfile {
        workspace: Some(workspace),
        exec_subpaths: vec!["/usr/bin".into(), text(&developer).to_owned()],
        ..ConfinementProfile::default()
    };
    let ended = run_to_end(
        &profile,
        &confinement::Command {
            argv: vec!["/usr/bin/env".into()],
            env: BTreeMap::new(),
        },
    )
    .await;
    assert_eq!(ended.code, Some(0), "{}", ended.report());
    let names: Vec<&str> = ended
        .stdout
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    assert!(
        names.contains(&"SDKROOT"),
        "control: the adapter set SDKROOT: {}",
        ended.report()
    );
    for name in [
        "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER",
        "CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER",
    ] {
        assert!(names.contains(&name), "{name} unset: {}", ended.report());
    }
}
