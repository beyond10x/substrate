//! A replacement whose write fails removes the parent directories it created
//! (story:failed-file-replacement-removes-the-directories-it-created).
//!
//! The rule: a replacement with `create_parents` that does not write its file leaves the
//! workspace as it was. The directories this call created are removed again, deepest first. A
//! directory that existed before the call stays, and so does one that holds an entry this call
//! did not create.
//!
//! The write these cases make fail is one the file system refuses by itself: a name of 256
//! bytes, one more than a directory entry holds.

use std::path::Path;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use substrate_wire::{Base64Content, Base64Encoding, ExpectedFileState, FileReplaceInput};
use tempfile::{TempDir, tempdir};

use super::GuardedFilesystem;
use crate::DriverError;

const WORKSPACE: &str = "ws";

/// A workspace `ws` holding `main.txt`, an empty directory `kept` and `full/held.txt`.
fn workspace() -> (TempDir, GuardedFilesystem) {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(
        filesystem.openat2_available(),
        "this case needs openat2; absent, it proves nothing and must not read as passed"
    );
    let root = directory.path().join(WORKSPACE);
    std::fs::create_dir(&root).expect("workspace root");
    std::fs::write(root.join("main.txt"), "seed\n").expect("seed file");
    std::fs::create_dir(root.join("kept")).expect("an empty directory");
    std::fs::create_dir(root.join("full")).expect("a directory");
    std::fs::write(root.join("full/held.txt"), "held\n").expect("a file in it");
    (directory, filesystem)
}

/// Every entry beneath the workspace, as its path from there, sorted; a directory ends in `/`.
fn listing(directory: &TempDir) -> Vec<String> {
    fn walk(root: &Path, directory: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("directory") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .expect("beneath the root")
                .to_string_lossy()
                .into_owned();
            if entry.file_type().expect("type").is_dir() {
                found.push(format!("{name}/"));
                walk(root, &path, found);
            } else {
                found.push(name);
            }
        }
    }
    let root = directory.path().join(WORKSPACE);
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    found
}

const AS_IT_WAS: [&str; 4] = ["full/", "full/held.txt", "kept/", "main.txt"];

fn creation() -> FileReplaceInput {
    FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode("x"),
        },
        expected: ExpectedFileState::Absent,
        create_parents: true,
    }
}

/// Asserts that creating `path` with its parents is refused and the workspace lists what it
/// listed before.
fn assert_refused_and_as_it_was(path: &str) {
    let (directory, filesystem) = workspace();
    assert_eq!(listing(&directory), AS_IT_WAS);
    let refusal = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, path, &creation())
        .expect_err("the file system refuses a name of 256 bytes");
    assert_eq!(refusal.code, "workspace.io-failed");
    assert_eq!(
        listing(&directory),
        AS_IT_WAS,
        "a refused creation of {path:?} changed the workspace"
    );
}

/// The write fails at the file's own name, after every parent exists: the parents this call
/// created are removed, in a workspace root, in a directory that was there before and in one
/// that holds a file.
#[test]
fn a_file_name_the_file_system_refuses_leaves_no_created_parent() {
    let over_long = "n".repeat(256);
    assert_refused_and_as_it_was(&format!("made/deeper/{over_long}"));
    assert_refused_and_as_it_was(&format!("kept/made/deeper/{over_long}"));
    assert_refused_and_as_it_was(&format!("full/made/{over_long}"));
    // No parent is created here, and none is removed.
    assert_refused_and_as_it_was(&format!("kept/{over_long}"));
}

/// The failure is at the last parent, while the parents are being created: the ones created
/// before it are removed.
#[test]
fn a_last_parent_the_file_system_refuses_leaves_no_created_parent() {
    let over_long = "n".repeat(256);
    assert_refused_and_as_it_was(&format!("made/deeper/{over_long}/file.txt"));
    assert_refused_and_as_it_was(&format!("kept/made/{over_long}/file.txt"));
    assert_refused_and_as_it_was(&format!("full/{over_long}/file.txt"));
}

/// A created directory that holds an entry this call did not create stays, and so does every
/// directory above it; the created ones beneath it are removed. The case creates the parents as
/// a replacement does, puts a file of another writer into the second of three, and removes.
#[test]
fn a_created_directory_that_holds_an_entry_of_another_writer_stays() {
    let (directory, filesystem) = workspace();
    let created = filesystem
        .create_parent_directories(WORKSPACE, "made/deeper/deepest/file.txt")
        .expect("three parents are created");
    let root = directory.path().join(WORKSPACE);
    std::fs::write(root.join("made/deeper/other.txt"), "other\n").expect("another writer's file");
    created.remove();
    assert_eq!(
        listing(&directory),
        [
            "full/",
            "full/held.txt",
            "kept/",
            "made/",
            "made/deeper/",
            "made/deeper/other.txt",
            "main.txt",
        ]
    );
}

/// A name that no longer is the directory this call created is not removed: the created
/// directory was moved away and another directory put under its name.
#[test]
fn a_directory_put_under_the_name_of_a_created_one_stays() {
    let (directory, filesystem) = workspace();
    let created = filesystem
        .create_parent_directories(WORKSPACE, "made/file.txt")
        .expect("one parent is created");
    let root = directory.path().join(WORKSPACE);
    std::fs::rename(root.join("made"), root.join("moved")).expect("the created one moved away");
    std::fs::rename(root.join("kept"), root.join("made")).expect("another directory in its place");
    created.remove();
    assert_eq!(
        listing(&directory),
        ["full/", "full/held.txt", "made/", "main.txt", "moved/"]
    );
}

/// A replacement that writes its file keeps the parents it created, and a later refused one
/// beneath them removes only its own.
#[test]
fn a_written_file_keeps_its_parents_and_a_later_failure_removes_only_its_own() {
    let (directory, filesystem) = workspace();
    filesystem
        .replace_cas(WORKSPACE, WORKSPACE, "made/deeper/file.txt", &creation())
        .expect("a replacement that is written");
    let written = [
        "full/",
        "full/held.txt",
        "kept/",
        "made/",
        "made/deeper/",
        "made/deeper/file.txt",
        "main.txt",
    ];
    assert_eq!(listing(&directory), written);
    filesystem
        .replace_cas(
            WORKSPACE,
            WORKSPACE,
            &format!("made/deeper/more/{}", "n".repeat(256)),
            &creation(),
        )
        .expect_err("the file system refuses a name of 256 bytes");
    assert_eq!(listing(&directory), written);
}

// The cases below hold the one retry of a replacement that asked for its parents. They run the
// steps `replace_cas` runs, in `write_when_expected`, and pass as the write a closure in which
// another writer removes the parents before the real `write_atomic`.

const NEW_FILE: &str = "made/deeper/file.txt";

/// Removes `made` and everything beneath it, as another writer would between this request's
/// creation of its parents and its write.
fn another_writer_removes_the_parents(directory: &TempDir) {
    std::fs::remove_dir_all(directory.path().join(WORKSPACE).join("made"))
        .expect("the parents are there to remove");
}

/// The parents are removed once: the request reads, creates and writes once more, and its file
/// is written. The same holds when the first write fails with another answer in a directory
/// that is gone.
#[test]
fn a_replacement_whose_parents_are_removed_once_is_written_by_its_one_retry() {
    for first_answer_is_the_writes_own in [true, false] {
        let (directory, filesystem) = workspace();
        let mut writes = 0;
        let (before, observation) = filesystem
            .write_when_expected(
                WORKSPACE,
                NEW_FILE,
                &ExpectedFileState::Absent,
                true,
                || {
                    writes += 1;
                    if writes > 1 {
                        return filesystem.write_atomic(WORKSPACE, WORKSPACE, NEW_FILE, b"x");
                    }
                    another_writer_removes_the_parents(&directory);
                    if first_answer_is_the_writes_own {
                        filesystem.write_atomic(WORKSPACE, WORKSPACE, NEW_FILE, b"x")
                    } else {
                        Err(DriverError::failed(
                            "workspace.io-failed",
                            "the temporary file was not created: its directory is gone",
                        ))
                    }
                },
            )
            .expect("the retry writes the file");
        assert_eq!(writes, 2);
        assert_eq!(before, None);
        assert_eq!(observation.size, 1);
        assert_eq!(
            listing(&directory),
            [
                "full/",
                "full/held.txt",
                "kept/",
                "made/",
                "made/deeper/",
                "made/deeper/file.txt",
                "main.txt",
            ]
        );
    }
}

/// The bound is one retry. Parents removed before the first write and again before the second
/// are `resource.not-found`, the refusal of a write whose directory is absent: two writes, no
/// third, and the workspace as it was. A request that did not ask for its parents is written
/// once, is refused the same way and creates nothing.
#[test]
fn parents_removed_twice_are_refused_after_one_retry_and_nothing_is_left() {
    let (directory, filesystem) = workspace();
    let mut writes = 0;
    let refusal = filesystem
        .write_when_expected(
            WORKSPACE,
            NEW_FILE,
            &ExpectedFileState::Absent,
            true,
            || {
                writes += 1;
                another_writer_removes_the_parents(&directory);
                filesystem.write_atomic(WORKSPACE, WORKSPACE, NEW_FILE, b"x")
            },
        )
        .expect_err("a second absence is the refusal");
    assert_eq!(refusal.code, "resource.not-found");
    assert_eq!(writes, 2);
    assert_eq!(listing(&directory), AS_IT_WAS);

    let mut writes = 0;
    let refusal = filesystem
        .write_when_expected(
            WORKSPACE,
            NEW_FILE,
            &ExpectedFileState::Absent,
            false,
            || {
                writes += 1;
                filesystem.write_atomic(WORKSPACE, WORKSPACE, NEW_FILE, b"x")
            },
        )
        .expect_err("no parent, and none asked for");
    assert_eq!(refusal.code, "resource.not-found");
    assert_eq!(writes, 1);
    assert_eq!(listing(&directory), AS_IT_WAS);
}

/// A second write that fails for another reason is the answer, and the directories the retry
/// created are removed like the first ones.
#[test]
fn a_retry_whose_write_fails_removes_the_parents_it_created() {
    let (directory, filesystem) = workspace();
    let mut writes = 0;
    let refusal = filesystem
        .write_when_expected(
            WORKSPACE,
            NEW_FILE,
            &ExpectedFileState::Absent,
            true,
            || {
                writes += 1;
                if writes == 1 {
                    another_writer_removes_the_parents(&directory);
                    return filesystem.write_atomic(WORKSPACE, WORKSPACE, NEW_FILE, b"x");
                }
                assert!(
                    directory
                        .path()
                        .join(WORKSPACE)
                        .join("made/deeper")
                        .is_dir(),
                    "the retry created the parents again"
                );
                Err(DriverError::failed("workspace.write-failed", "no space"))
            },
        )
        .expect_err("the second write fails");
    assert_eq!(refusal.code, "workspace.write-failed");
    assert_eq!(writes, 2);
    assert_eq!(listing(&directory), AS_IT_WAS);
}

/// The retry skips no check. A replacement of a file with an expected digest whose directory
/// another writer removes, with the file, is not written again: the retry reads the file,
/// finds it absent and answers `workspace.stale-content`, with one write made and no directory
/// created.
#[test]
fn a_retry_verifies_the_precondition_again_before_it_creates_or_writes() {
    let (directory, filesystem) = workspace();
    let root = directory.path().join(WORKSPACE);
    let expected = ExpectedFileState::Sha256 {
        sha256: hex::encode(Sha256::digest(b"held\n")),
    };
    let mut writes = 0;
    let refusal = filesystem
        .write_when_expected(WORKSPACE, "full/held.txt", &expected, true, || {
            writes += 1;
            std::fs::remove_dir_all(root.join("full")).expect("another writer removes it");
            filesystem.write_atomic(WORKSPACE, WORKSPACE, "full/held.txt", b"x")
        })
        .expect_err("the file the precondition names is gone");
    assert_eq!(refusal.code, "workspace.stale-content");
    assert_eq!(writes, 1);
    assert_eq!(listing(&directory), ["kept/", "main.txt"]);
}
