//! Adversarial cases for the rule that a replacement whose write fails removes the directories
//! it created and nothing else
//! (story:failed-file-replacement-removes-the-directories-it-created).
//!
//! Each case runs the two steps of a replacement with `create_parents` as `replace_cas` runs
//! them, `create_parent_directories` and then `write_atomic`, with another request's step
//! between them.

use std::os::unix::fs::symlink;

use substrate_wire::ExpectedFileState;
use tempfile::{TempDir, tempdir};

use super::GuardedFilesystem;

const WORKSPACE: &str = "ws";

fn workspace() -> (TempDir, GuardedFilesystem) {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(filesystem.openat2_available(), "this case needs openat2");
    std::fs::create_dir(directory.path().join(WORKSPACE)).expect("workspace root");
    (directory, filesystem)
}

/// Two replacements with `create_parents` beneath one new directory at the same time. The first
/// creates `made`, the second finds it there, the first one's write fails and its cleanup runs,
/// then the second writes. The second asked for its parents and its own write does not fail, so
/// its file is written: the cleanup of one request does not refuse another.
///
/// The second request runs its steps as `replace_cas` runs them, in `write_when_expected`: its
/// precondition, its parents, its write. The first one's cleanup runs where the case puts it,
/// after the second has found `made` and before the second's first `write_atomic`.
#[test]
fn a_replacement_that_found_its_parent_is_written_after_another_ones_cleanup() {
    let (directory, filesystem) = workspace();
    let failing = filesystem
        .create_parent_directories(WORKSPACE, "made/deeper/file.txt")
        .expect("the first request creates two parents");
    let mut failing = Some(failing);
    let mut found_before_the_cleanup = false;
    let written = filesystem.write_when_expected(
        WORKSPACE,
        "made/other.txt",
        &ExpectedFileState::Absent,
        true,
        || {
            if let Some(failing) = failing.take() {
                found_before_the_cleanup = directory.path().join("ws/made").is_dir();
                failing.remove();
                assert!(!directory.path().join("ws/made").exists(), "the cleanup");
            }
            filesystem.write_atomic(WORKSPACE, WORKSPACE, "made/other.txt", b"x")
        },
    );
    assert!(
        found_before_the_cleanup,
        "the second request found its parent"
    );
    assert!(
        written.is_ok(),
        "the second request asked for its parents and was refused: {:?}",
        written.err().map(|refusal| refusal.code)
    );
    assert_eq!(
        std::fs::read(directory.path().join("ws/made/other.txt")).expect("its file"),
        b"x"
    );
}

/// The created directory is moved away and a symbolic link to it is put under its name. The
/// cleanup removes neither the link nor the directory it names, and nothing beneath it.
#[test]
fn a_symbolic_link_put_under_the_name_of_a_created_directory_stays_with_what_it_names() {
    let (directory, filesystem) = workspace();
    let root = directory.path().join(WORKSPACE);
    let created = filesystem
        .create_parent_directories(WORKSPACE, "made/file.txt")
        .expect("one parent is created");
    std::fs::rename(root.join("made"), root.join("moved")).expect("the created one moved away");
    symlink("moved", root.join("made")).expect("a link under its name");
    created.remove();
    assert!(root.join("moved").is_dir(), "the directory the link names");
    assert!(
        std::fs::symlink_metadata(root.join("made"))
            .expect("the link")
            .file_type()
            .is_symlink()
    );
}

/// A middle parent that was there before the request stays, with the entry it held, when the
/// deeper one this request created is removed; so does the workspace root.
#[test]
fn a_parent_that_was_there_before_stays_when_the_one_created_beneath_it_is_removed() {
    let (directory, filesystem) = workspace();
    let root = directory.path().join(WORKSPACE);
    std::fs::create_dir(root.join("was")).expect("a directory that was there");
    let created = filesystem
        .create_parent_directories(WORKSPACE, "was/made/deeper/file.txt")
        .expect("two parents are created");
    created.remove();
    assert!(root.join("was").is_dir());
    assert!(!root.join("was/made").exists());
    let created = filesystem
        .create_parent_directories(WORKSPACE, "made/file.txt")
        .expect("one parent is created");
    created.remove();
    assert!(root.is_dir(), "the workspace root");
    assert_eq!(std::fs::read_dir(&root).expect("root").count(), 1);
}
