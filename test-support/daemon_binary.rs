//! The shipped `substrate-daemon` a test starts: found, never built by a test.
//!
//! In order: the program `SUBSTRATE_TEST_DAEMON` names, then the one beside the test executable.
//! A workspace test build leaves that one there before any test runs: it is the setup step of
//! the gate's test run (AGENTS.md, section *The gate*). A test that finds neither fails and names
//! the step; it starts no build.
//!
//! `story:gate-test-step-ends-within-two-minutes`: one of four copies of this lookup skipped the
//! sibling and ran `cargo build -p b10x-substrate-daemon` inside its test. That package-scoped
//! build resolves other features than the workspace's test build, so it recompiled the daemon and
//! everything beneath it, 95 s on the hosted runner, inside one test, and replaced the daemon
//! every later test started. Under cargo-nextest every test is its own process, so a build kept
//! in a `OnceLock` would run once per test. The lookup is this one file so that no copy can
//! differ again.
//!
//! Included with `#[path]` from a test two directories below the workspace root
//! (`crates/<crate>/tests/`); it is test support, never part of a production path.
#![allow(
    dead_code,
    reason = "each including test binary uses the part it needs and nothing else"
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The daemon's file name beside a test executable.
const DAEMON: &str = "substrate-daemon";

/// The shipped daemon for this test process.
///
/// # Panics
///
/// When no daemon is named and none is beside this test executable.
pub fn daemon_binary() -> PathBuf {
    let test_executable = std::env::current_exe().expect("current test executable");
    resolve(std::env::var_os("SUBSTRATE_TEST_DAEMON"), &test_executable).unwrap_or_else(|absent| {
        panic!(
            "the shipped daemon is missing at {}: a workspace test build leaves it there \
             (`cargo nextest run --workspace --no-run`, with the profile of this test), or \
             SUBSTRATE_TEST_DAEMON names one; a test builds none",
            absent.display()
        )
    })
}

/// The directory cargo places a profile's binaries in: the test executable's own, or its parent
/// when the executable is in `deps/`.
pub fn profile_dir(test_executable: &Path) -> PathBuf {
    let mut directory = test_executable.to_path_buf();
    directory.pop();
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory.pop();
    }
    directory
}

/// `named` when given; otherwise the daemon beside `test_executable`.
///
/// # Errors
///
/// The path the daemon would have beside the test executable, when it is no file.
pub fn resolve(named: Option<OsString>, test_executable: &Path) -> Result<PathBuf, PathBuf> {
    if let Some(named) = named {
        return Ok(named.into());
    }
    let sibling = profile_dir(test_executable).join(DAEMON);
    if sibling.is_file() {
        Ok(sibling)
    } else {
        Err(sibling)
    }
}
