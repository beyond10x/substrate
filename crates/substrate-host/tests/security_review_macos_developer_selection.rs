// Security review conformance case for `story:confined-cargo-build-links-without-the-xcrun-shim`:
// the developer tools a confined build links with when the selection changes after the macos
// adapter opened.
//
// The case switches the host's `xcode-select` selection with `sudo -n` and restores it before it
// returns, so it runs alone in this test binary: no other case can observe the switched selection.
// It needs a second Xcode in `/Applications` and password-free `sudo`, as the macos-15 runner has;
// without either it fails naming the missing prerequisite, never passes.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as HostCommand;
use std::time::Duration;

use substrate_host::confinement::{
    self, AdapterRequest, ConfinementProfile, RunState, macos::MacosRequest,
};
use substrate_host::seatbelt;

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

/// The developer directory `xcode-select` names now.
fn selected_directory() -> String {
    let output = HostCommand::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .expect("xcode-select runs");
    assert!(output.status.success(), "xcode-select -p: {output:?}");
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

fn select(directory: &str) {
    let output = HostCommand::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/xcode-select", "-s", directory])
        .output()
        .expect("sudo runs");
    assert!(
        output.status.success(),
        "prerequisite: password-free sudo to run xcode-select -s {directory}: {output:?}"
    );
}

/// Restores the selection it was made with when the case ends, passed or not.
struct Restore(String);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = HostCommand::new("/usr/bin/sudo")
            .args(["-n", "/usr/bin/xcode-select", "-s", &self.0])
            .status();
    }
}

/// An Xcode in `/Applications` other than the selected developer root `selected`.
fn another_xcode(selected: &Path) -> PathBuf {
    fs::read_dir("/Applications")
        .expect("/Applications")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("Xcode") && name.ends_with(".app")
        })
        .filter_map(|entry| fs::canonicalize(entry.path()).ok())
        .find(|bundle| bundle != selected && bundle.join("Contents/Developer").is_dir())
        .expect("prerequisite: a second Xcode in /Applications")
}

// The module documentation states it: "a start whose exec subpaths hold the whole developer root
// gets `SDKROOT` and the host target's `CARGO_TARGET_<TRIPLE>_LINKER` naming them … When the open
// could not resolve them, such a start is refused `exec.sandbox-developer-tools-unresolved` and
// nothing runs." Developer tools selected after the open were never resolved, and a start whose
// exec subpath is their root is either refused by that name or given an SDK and a linker beneath
// that root; it never runs with neither, where cargo links through the `/usr/bin/cc` shim and fails
// with exit 72, the failure the story closes.
#[tokio::test(flavor = "multi_thread")]
async fn developer_tools_selected_after_open_are_refused_or_given_to_a_build() {
    let adapter = confinement::open(AdapterRequest::Macos(MacosRequest::default()))
        .expect("macos opens on a macOS host whose Seatbelt sandbox enforces");
    let opened = seatbelt::developer_root().expect("the runner has developer tools");
    let other = another_xcode(&opened);
    let _restore = Restore(selected_directory());
    select(text(&other.join("Contents/Developer")));
    assert_eq!(
        seatbelt::developer_root().as_deref(),
        Some(other.as_path()),
        "the switched selection"
    );
    let root = tempfile::tempdir().expect("workspace");
    let workspace = fs::canonicalize(root.path()).expect("canonical workspace");
    let profile = ConfinementProfile {
        workspace: Some(workspace),
        exec_subpaths: vec![text(&other).to_owned()],
        ..ConfinementProfile::default()
    };
    let linker = format!(
        "CARGO_TARGET_{}_APPLE_DARWIN_LINKER",
        std::env::consts::ARCH.to_ascii_uppercase()
    );
    let command = confinement::Command {
        argv: vec![
            "/bin/bash".into(),
            "-c".into(),
            format!("printf '%s\\n%s\\n' \"${{SDKROOT-unset}}\" \"${{{linker}-unset}}\""),
        ],
        env: BTreeMap::new(),
    };
    let admitted = adapter.admit(&profile).expect("admitted");
    let run = match adapter.start(&admitted, &command).await {
        Err(error) => {
            assert_eq!(
                error.code(),
                seatbelt::DEVELOPER_TOOLS_UNRESOLVED,
                "{error}"
            );
            return;
        }
        Ok(run) => run,
    };
    let ended = tokio::time::timeout(Duration::from_secs(45), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let output = adapter.output(&run).await.expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout:?}");
    for (name, value) in ["SDKROOT", linker.as_str()].into_iter().zip(lines) {
        assert!(
            Path::new(value).starts_with(&other),
            "a start whose exec subpath is the developer tools selected after the open, {}, ran \
             with {name}={value} and no refusal (opened over {})",
            text(&other),
            text(&opened)
        );
    }
}
