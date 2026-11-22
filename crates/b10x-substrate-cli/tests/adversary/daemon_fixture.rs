//! The daemon lookup the CLI's test binaries share (`test-support/daemon_binary.rs`) finds the
//! daemon and never builds one (`story:gate-test-step-ends-within-two-minutes`): the daemon a
//! workspace build left beside the test is used as it is, and an absent one is an error that
//! names where it would be.

use std::path::{Path, PathBuf};

use super::daemon_binary;

/// A cargo profile directory with a test executable in its `deps/`, as cargo lays one out.
struct Profile {
    _directory: tempfile::TempDir,
    root: PathBuf,
    test_executable: PathBuf,
}

fn profile() -> Profile {
    let directory = tempfile::tempdir().expect("profile directory");
    let root = directory.path().join("release");
    std::fs::create_dir_all(root.join("deps")).expect("deps directory");
    let test_executable = root.join("deps/adversary-0123456789abcdef");
    std::fs::write(&test_executable, b"").expect("test executable");
    Profile {
        _directory: directory,
        root,
        test_executable,
    }
}

#[test]
fn the_daemon_beside_the_test_is_the_one_a_test_starts() {
    let profile = profile();
    let sibling = profile.root.join("substrate-daemon");
    std::fs::write(&sibling, b"").expect("sibling daemon");
    assert_eq!(
        daemon_binary::resolve(None, &profile.test_executable),
        Ok(sibling.clone())
    );
    // An executable placed directly in the profile directory has the same sibling.
    assert_eq!(
        daemon_binary::resolve(None, &profile.root.join("adversary")),
        Ok(sibling)
    );
}

#[test]
fn a_named_daemon_wins_over_the_one_beside_the_test() {
    let profile = profile();
    std::fs::write(profile.root.join("substrate-daemon"), b"").expect("sibling daemon");
    assert_eq!(
        daemon_binary::resolve(
            Some("/named/substrate-daemon".into()),
            &profile.test_executable
        ),
        Ok(Path::new("/named/substrate-daemon").to_path_buf())
    );
}

#[test]
fn an_absent_daemon_is_an_error_naming_where_it_would_be_and_nothing_is_built() {
    let profile = profile();
    let sibling = profile.root.join("substrate-daemon");
    assert_eq!(
        daemon_binary::resolve(None, &profile.test_executable),
        Err(sibling.clone())
    );
    // A directory of that name is no daemon either.
    std::fs::create_dir(&sibling).expect("a directory in the daemon's place");
    assert_eq!(
        daemon_binary::resolve(None, &profile.test_executable),
        Err(sibling)
    );
    let left: Vec<_> = std::fs::read_dir(&profile.root)
        .expect("profile directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(left.len(), 2, "the lookup wrote into the profile: {left:?}");
}
