//! Cases of the guards that keep a baseline write and a baseline removal from what stands in
//! their way (design 21, § *The baseline store*): a store and a commit-id file are never put
//! over an entry that stands, and a link in place of the directory of the stores is neither
//! written through nor removed through. Each case puts the entry in place before the one call
//! it makes, so none depends on when another thread runs.

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use git2::Oid;
use tempfile::tempdir;

use super::super::{remove_baseline, store, write_commit_id};
use super::{source_repository, write_baseline};
use crate::git_baselines::DEFAULT_LIMIT;

/// The names in `directory`, sorted.
fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .expect("directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The store of a create is renamed into place without replacing what stands there: a store
/// that holds nothing and a file are both left as they are, the write ends as one whose root
/// name has a store, and its staging directory is gone. A create asks whether the root name is
/// taken before it writes, so only an entry that appears after that question reaches this
/// rename; the write is called here with the entry already in place.
#[test]
fn a_store_is_not_renamed_over_an_entry_that_stands_in_its_place() {
    let (workspace, commit) = source_repository();
    for what in ["a store that holds nothing", "a file"] {
        let baselines = tempdir().expect("baseline root");
        let stores = baselines.path().join(".objects");
        std::fs::create_dir(&stores).expect("directory of the stores");
        let place = stores.join("ws_git");
        if what == "a file" {
            std::fs::write(&place, b"another workspace's\n").expect("file");
        } else {
            std::fs::create_dir(&place).expect("store");
        }
        let entry = |path: &Path| {
            let metadata = path.symlink_metadata().expect("entry");
            (metadata.ino(), metadata.is_dir(), metadata.len())
        };
        let before = entry(&place);

        let written = store::write(
            baselines.path(),
            &place,
            &workspace.path().join(".git/objects"),
            commit,
            DEFAULT_LIMIT,
        );

        assert_eq!(written, Err(store::Unwritten::Exists), "{what}");
        assert_eq!(entry(&place), before, "{what}");
        if what == "a file" {
            assert_eq!(
                std::fs::read(&place).expect("file"),
                b"another workspace's\n"
            );
        } else {
            assert_eq!(names(&place), Vec::<String>::new(), "{what}");
        }
        assert_eq!(names(&stores), ["ws_git"], "{what}");
        assert_eq!(names(baselines.path()), [".objects"], "{what}");
    }
}

/// The commit-id file of a create is put into place without replacing one that stands: the
/// write is refused `workspace.git-target-exists`, the file that stands keeps its bytes, and
/// the staged file is gone.
#[test]
fn a_commit_id_file_is_not_put_over_one_that_stands() {
    let baselines = tempdir().expect("baseline root");
    let place = baselines.path().join("ws_git");
    std::fs::write(&place, b"another workspace's\n").expect("commit-id file");
    let commit = Oid::from_bytes(&[7; 20]).expect("commit id");

    let error = write_commit_id(baselines.path(), &place, commit)
        .expect_err("the root name has a commit-id file");

    assert_eq!(error.code, "workspace.git-target-exists");
    assert_eq!(
        std::fs::read(&place).expect("commit-id file"),
        b"another workspace's\n"
    );
    assert_eq!(names(baselines.path()), ["ws_git"]);
}

/// A link in place of the directory of the stores is not followed when a store is put into
/// place: the write is refused `workspace.git-baseline-write-failed`, nothing is written
/// where the link leads, and neither a commit-id file nor a staging directory is left.
#[test]
fn a_store_is_not_written_through_a_link_in_place_of_the_directory_of_the_stores() {
    let (workspace, commit) = source_repository();
    let baselines = tempdir().expect("baseline root");
    let elsewhere = tempdir().expect("directory outside the baseline root");
    let stores = baselines.path().join(".objects");
    std::os::unix::fs::symlink(elsewhere.path(), &stores).expect("link");

    let error = write_baseline(
        baselines.path(),
        "ws_git",
        workspace.path(),
        &commit.to_string(),
        u64::MAX,
    )
    .expect_err("the directory of the stores is a link");

    assert_eq!(error.code, "workspace.git-baseline-write-failed");
    assert_eq!(
        names(elsewhere.path()),
        Vec::<String>::new(),
        "a store was written where the link leads"
    );
    assert!(stores.symlink_metadata().expect("link").is_symlink());
    assert_eq!(names(baselines.path()), [".objects"]);
}

/// A link in place of the directory of the stores is not followed when a baseline is removed:
/// the commit-id file goes, and the directory of the root name where the link leads stays
/// with everything in it.
#[test]
fn a_baseline_removal_removes_nothing_through_a_link_in_place_of_the_directory_of_the_stores() {
    let baselines = tempdir().expect("baseline root");
    let elsewhere = tempdir().expect("directory outside the baseline root");
    let outside = elsewhere.path().join("ws_git/pack");
    std::fs::create_dir_all(&outside).expect("directory of the root name outside");
    std::fs::write(outside.join("kept"), b"kept\n").expect("file outside");
    let stores = baselines.path().join(".objects");
    std::os::unix::fs::symlink(elsewhere.path(), &stores).expect("link");
    std::fs::write(baselines.path().join("ws_git"), b"commit id\n").expect("commit-id file");

    remove_baseline(baselines.path(), "ws_git").expect("the baseline is removed");

    assert_eq!(names(baselines.path()), [".objects"]);
    assert!(stores.symlink_metadata().expect("link").is_symlink());
    assert_eq!(names(elsewhere.path()), ["ws_git"]);
    assert_eq!(
        std::fs::read(outside.join("kept")).expect("file outside"),
        b"kept\n"
    );
}
