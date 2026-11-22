//! Cases of the baseline store (design 21, § *The baseline store*) where a write or a read is
//! refused: by which name, and what stands in the baseline root afterwards.

use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use git2::{Oid, Repository, Signature};
use sha2::{Digest as _, Sha256};
use tempfile::{TempDir, tempdir};

use super::store_cases::{leftovers, plant_store, store_objects};
use super::{
    baseline_file, baseline_object, baseline_root, commit_paths, objects_of, observe,
    source_repository, store_of, target, write_baseline,
};
use crate::{DriverError, DriverErrorClass};

/// How deep the directories of a baseline may nest (design 21, § *Nesting*).
pub(in crate::git) const NESTING_BOUND: usize = 1024;

/// The tree of a directory in which `levels - 1` directories nest in each other, the innermost
/// empty: with itself, `levels` directories. The trees are written to the object database, so
/// no directory is made for them.
fn nested(repository: &Repository, levels: usize) -> Oid {
    let mut innermost = repository
        .treebuilder(None)
        .and_then(|builder| builder.write())
        .expect("empty directory");
    for _ in 1..levels {
        innermost = holding(repository, innermost);
    }
    innermost
}

/// The tree of a directory that holds the directory `directory` as `d`, and nothing else.
fn holding(repository: &Repository, directory: Oid) -> Oid {
    let mut builder = repository.treebuilder(None).expect("tree builder");
    builder
        .insert("d", directory, 0o040_000)
        .expect("directory");
    builder.write().expect("directory")
}

/// A repository whose branch `provider-default` holds one commit, of the tree `root` writes.
pub(in crate::git) fn repository_holding(root: impl FnOnce(&Repository) -> Oid) -> (TempDir, Oid) {
    let directory = tempdir().expect("repository directory");
    let repository = Repository::init(directory.path()).expect("repository");
    let root = root(&repository);
    let signature =
        Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
    let commit = repository
        .commit(
            Some("refs/heads/provider-default"),
            &signature,
            &signature,
            "fixture",
            &repository.find_tree(root).expect("root tree"),
            &[],
        )
        .expect("commit");
    repository
        .set_head("refs/heads/provider-default")
        .expect("provider HEAD");
    drop(repository);
    (directory, commit)
}

/// A repository whose branch `provider-default` holds one commit: the file `README.md`, and
/// the directories `directories` writes, by name.
pub(super) fn repository_of(
    directories: impl FnOnce(&Repository) -> Vec<(String, Oid)>,
) -> (TempDir, Oid) {
    repository_holding(|repository| {
        let file = repository.blob(b"bounded git workspace\n").expect("file");
        let mut top = repository.treebuilder(None).expect("tree builder");
        top.insert("README.md", file, 0o100_644).expect("file");
        for (name, tree) in directories(repository) {
            top.insert(name, tree, 0o040_000).expect("directory");
        }
        top.write().expect("root tree")
    })
}

/// A repository whose branch `provider-default` holds one commit: a file, and `levels`
/// directories nested in each other.
pub(in crate::git) fn nested_repository(levels: usize) -> (TempDir, Oid) {
    repository_of(|repository| vec![("d".to_owned(), nested(repository, levels))])
}

/// Copies the directory `from` and everything in it to `to`, which does not exist yet.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir(to).expect("directory");
    for entry in std::fs::read_dir(from).expect("directory") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("entry type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("file");
        }
    }
}

/// The baseline root of a workspace materialized before the store existed, whose commit-id file
/// names `commit`.
pub(super) fn commit_id_file_alone(commit: Oid) -> TempDir {
    let baselines = tempdir().expect("baseline root");
    std::fs::write(baselines.path().join("ws_git"), format!("{commit}\n")).expect("commit-id file");
    baselines
}

/// Both observations of the workspace: the changes, then the baseline's `README.md`.
pub(super) fn both(workspace: &Path, baselines: &Path) -> [Result<(), DriverError>; 2] {
    [
        observe(workspace, baselines, 1024).map(|_| ()),
        target(workspace, baselines, |held| {
            baseline_file(held, "README.md", 1024)
        })
        .map(|_| ()),
    ]
}

pub(super) fn codes(outcomes: [Result<(), DriverError>; 2]) -> [&'static str; 2] {
    outcomes.map(|outcome| outcome.map_or_else(|error| error.code, |()| "answered"))
}

/// A directory the baseline holds in two places is bounded at each of them: the same
/// directories are within the bound as `d`, and one level deeper, as `a/d`, they are not when
/// that level is one too many. The refused write leaves nothing in the baseline root.
#[test]
fn a_directory_the_baseline_holds_twice_is_bounded_at_its_deeper_place() {
    let twice = |levels: usize| {
        repository_of(|repository| {
            let nested = nested(repository, levels);
            vec![
                ("a".to_owned(), holding(repository, nested)),
                ("d".to_owned(), nested),
            ]
        })
    };
    let write = |levels: usize| {
        let (repository, commit) = twice(levels);
        let baselines = tempdir().expect("baseline root");
        let written = write_baseline(
            baselines.path(),
            "ws_git",
            repository.path(),
            &commit.to_string(),
            u64::MAX,
        );
        (written, baselines)
    };

    let (written, baselines) = write(NESTING_BOUND - 1);
    written.expect("a baseline whose directories nest as deep as the bound");
    // The commit, its tree, the file, `a` and one tree for each directory nested as `d`.
    assert_eq!(
        objects_of(&store_of(baselines.path())).len(),
        NESTING_BOUND + 3
    );

    let (written, baselines) = write(NESTING_BOUND);
    assert_eq!(
        written.expect_err("one level too many").code,
        "workspace.git-path-unrepresentable"
    );
    assert_eq!(
        std::fs::read_dir(baselines.path())
            .expect("baseline root")
            .count(),
        0,
        "the refused write left an entry"
    );
}

/// Takes every permission of the entry at `path` from this user, so opening it fails. `None`
/// for a user that opens it all the same; the entry has `restored` again then.
fn closed(path: &Path, mode: u32, restored: u32) -> Option<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("entry mode");
    if std::fs::File::open(path).is_ok() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(restored))
            .expect("entry mode");
        return None;
    }
    Some(())
}

const NOT_RUN: &str = "absent, not run: this user opens an entry whose mode forbids it";

/// A failure leaves neither entry of its create (`write_baseline`): also a failure after the
/// store was renamed into place, here the sync of the directory of the stores.
#[test]
fn a_baseline_write_that_fails_once_its_store_is_in_place_leaves_no_store() {
    let (workspace, commit) = source_repository();
    let baselines = tempdir().expect("baseline root");
    let stores = baselines.path().join(".objects");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&stores)
        .expect("directory of the stores");
    closed(&stores, 0o300, 0o700).expect(NOT_RUN);

    let outcome = write_baseline(
        baselines.path(),
        "ws_git",
        workspace.path(),
        &commit.to_string(),
        u64::MAX,
    );
    std::fs::set_permissions(&stores, std::fs::Permissions::from_mode(0o700))
        .expect("directory mode");

    let error = outcome.expect_err("the directory of the stores cannot be synced");
    assert_eq!(error.code, "workspace.git-baseline-write-failed");
    assert!(!baselines.path().join("ws_git").exists());
    assert!(
        !store_of(baselines.path()).exists(),
        "the failed write left its store"
    );
}

/// A failure leaves neither entry of its create (`write_baseline`): also a failure after the
/// commit-id file was put into place, here the sync of the baseline root.
#[test]
fn a_baseline_write_that_fails_once_its_commit_id_file_is_in_place_leaves_no_commit_id_file() {
    let (workspace, commit) = source_repository();
    let baselines = tempdir().expect("baseline root");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(baselines.path().join(".objects"))
        .expect("directory of the stores");
    closed(baselines.path(), 0o300, 0o700).expect(NOT_RUN);

    let outcome = write_baseline(
        baselines.path(),
        "ws_git",
        workspace.path(),
        &commit.to_string(),
        u64::MAX,
    );
    std::fs::set_permissions(baselines.path(), std::fs::Permissions::from_mode(0o700))
        .expect("directory mode");

    let error = outcome.expect_err("the baseline root cannot be synced");
    assert_eq!(error.code, "workspace.git-baseline-write-failed");
    assert!(!store_of(baselines.path()).exists());
    assert!(
        !baselines.path().join("ws_git").exists(),
        "the failed write left its commit-id file"
    );
}

/// A workspace whose commit-id file names its second commit, and whose store holds only its
/// first: the store does not hold the named commit.
pub(super) fn store_of_another_commit() -> (TempDir, TempDir, Oid) {
    let (workspace, first) = source_repository();
    std::fs::write(workspace.path().join("later.txt"), b"later\n").expect("file");
    let commit = commit_paths(workspace.path(), first, &["later.txt"]);
    let baselines = commit_id_file_alone(commit);
    plant_store(baselines.path(), workspace.path(), first);
    assert!(!store_objects(baselines.path()).contains(&commit));
    (workspace, baselines, commit)
}

/// A store that does not hold the commit the commit-id file names gives no observation: both
/// are refused `workspace.git-baseline-missing`, and the store is there afterwards as it was,
/// though the workspace's own object store holds every object of the named commit.
#[test]
fn a_store_that_lacks_the_named_commit_is_refused_and_left_as_it_is() {
    let (workspace, baselines, commit) = store_of_another_commit();
    assert!(
        objects_of(&workspace.path().join(".git/objects")).contains(&commit),
        "the workspace holds the named commit"
    );
    let before = listing(baselines.path());

    assert_eq!(
        codes(both(workspace.path(), baselines.path())),
        ["workspace.git-baseline-missing"; 2]
    );
    assert_eq!(listing(baselines.path()), before);
    assert!(!store_objects(baselines.path()).contains(&commit));
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}

/// The entries of the directory of the stores, each with its type, its mode and the time it
/// was last written, and a regular file also with its length and the hash of its bytes; sorted.
fn listing(baselines: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let mut pending = vec![baselines.join(".objects")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("directory") {
            let path = entry.expect("entry").path();
            let metadata = path.symlink_metadata().expect("entry");
            let content = if metadata.is_file() {
                let bytes = std::fs::read(&path).expect("file of the store");
                format!(" {} {}", bytes.len(), hex::encode(Sha256::digest(&bytes)))
            } else {
                String::new()
            };
            entries.push(format!(
                "{} {:?} {:o} {}.{:09}{content}",
                path.display(),
                metadata.file_type(),
                metadata.permissions().mode() & 0o7777,
                metadata.mtime(),
                metadata.mtime_nsec()
            ));
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    entries.sort();
    entries
}

/// A store that cannot be opened or read is not absent: both observations are refused
/// `workspace.git-baseline-read-failed`, and every entry of it is there afterwards as it was.
#[test]
fn a_store_that_cannot_be_read_is_refused_and_not_removed() {
    let (workspace, commit) = source_repository();
    // The store itself, its pack directory, and each file of its pack: the same names in every
    // store of this commit.
    let entries: Vec<PathBuf> = {
        let baselines = baseline_root(workspace.path(), commit);
        let files = std::fs::read_dir(store_of(baselines.path()).join("pack"))
            .expect("pack directory")
            .map(|entry| Path::new("pack").join(entry.expect("entry").file_name()));
        [PathBuf::new(), PathBuf::from("pack")]
            .into_iter()
            .chain(files)
            .collect()
    };
    assert_eq!(entries.len(), 4, "{entries:?}");
    for relative in entries {
        let baselines = baseline_root(workspace.path(), commit);
        let entry = store_of(baselines.path()).join(&relative);
        let before = listing(baselines.path());
        let mode = entry.metadata().expect("entry").permissions().mode() & 0o7777;
        closed(&entry, 0o000, mode).expect(NOT_RUN);

        let outcomes = both(workspace.path(), baselines.path());
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(mode))
            .expect("entry mode");

        assert_eq!(
            codes(outcomes),
            ["workspace.git-baseline-read-failed"; 2],
            "{}",
            entry.display()
        );
        assert_eq!(listing(baselines.path()), before, "{}", entry.display());
        assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
    }
}

/// A store whose pack index the Git library cannot read as one gives no observation: both are
/// refused `workspace.git-baseline-missing`, and the store is there afterwards as it was,
/// though the workspace's own object store holds every object of the commit.
#[test]
fn a_store_whose_pack_index_is_not_an_index_is_refused_and_left_as_it_is() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let index = std::fs::read_dir(store_of(baselines.path()).join("pack"))
        .expect("pack directory")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
        .expect("the index of the pack");
    let length = usize::try_from(index.metadata().expect("index").len()).expect("length");
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o600)).expect("mode");
    std::fs::write(&index, vec![b'x'; length]).expect("bytes that are no index");
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o400)).expect("mode");
    let before = listing(baselines.path());
    assert!(baseline_object(workspace.path(), commit).is_file());

    assert_eq!(
        codes(both(workspace.path(), baselines.path())),
        ["workspace.git-baseline-missing"; 2]
    );
    assert_eq!(listing(baselines.path()), before);
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}

/// What stands in a store's place and is not a directory is not absent either: a file, and a
/// link to a store, are refused `workspace.git-baseline-read-failed` and left as they are.
#[test]
fn an_entry_in_the_place_of_a_store_that_is_no_directory_is_refused_and_not_removed() {
    let (workspace, commit) = source_repository();
    let elsewhere = baseline_root(workspace.path(), commit);
    for what in ["a file", "a link to a store"] {
        let baselines = commit_id_file_alone(commit);
        std::fs::create_dir(baselines.path().join(".objects")).expect("directory of the stores");
        let store = store_of(baselines.path());
        if what == "a file" {
            std::fs::write(&store, b"not a store\n").expect("file");
        } else {
            std::os::unix::fs::symlink(store_of(elsewhere.path()), &store).expect("link");
        }
        let before = listing(baselines.path());

        assert_eq!(
            codes(both(workspace.path(), baselines.path())),
            ["workspace.git-baseline-read-failed"; 2],
            "{what}"
        );
        assert_eq!(listing(baselines.path()), before, "{what}");
        assert_eq!(leftovers(baselines.path()), Vec::<String>::new(), "{what}");
    }
}

/// A baseline file whose object states a size over `max_bytes` is refused
/// `workspace.git-baseline-limit` from that statement: the object is not read. The object's
/// file in this store states 4,096 bytes and holds bytes that are not the object's, so reading
/// it fails its check, which the limit's refusal shows did not happen.
#[test]
fn a_baseline_file_stated_larger_than_the_limit_is_refused_before_its_object_is_read() {
    let (workspace, commit) = source_repository();
    let baselines = commit_id_file_alone(commit);
    std::fs::create_dir(baselines.path().join(".objects")).expect("directory of the stores");
    let objects = workspace.path().join(".git/objects");
    let larger = Repository::open(workspace.path())
        .expect("repository")
        .blob(&[b'x'; 4096])
        .expect("another object")
        .to_string();
    copy_tree(&objects, &store_of(baselines.path()));
    let object = baseline_object(workspace.path(), commit);
    let held = store_of(baselines.path()).join(object.strip_prefix(&objects).expect("object"));
    std::fs::remove_file(&held).expect("object removed");
    std::fs::copy(objects.join(&larger[..2]).join(&larger[2..]), &held)
        .expect("another object's bytes in the object's place");
    let read = |max_bytes: u64| {
        target(workspace.path(), baselines.path(), |held| {
            baseline_file(held, "README.md", max_bytes)
        })
        .expect_err("no answer from this object")
    };

    let over = read(4095);
    assert_eq!(over.code, "workspace.git-baseline-limit");
    assert_eq!(over.class, DriverErrorClass::Exhausted);
    assert_eq!(read(4096).code, "workspace.git-baseline-read-failed");
}
