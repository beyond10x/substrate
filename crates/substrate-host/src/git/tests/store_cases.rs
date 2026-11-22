//! Cases of the baseline store (design 21, § *The baseline store*): the driver keeps the objects
//! of a Git workspace's baseline commit in a store of its own, both observations read the
//! baseline from that store, and a workspace that predates the store gets one at its first
//! observation.

use std::collections::BTreeSet;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;
use std::time::Duration;

use git2::{ObjectType, Oid, Repository, Signature};
use sha2::{Digest as _, Sha256};
use substrate_wire::{GitBaselineFile, GitChange, GitChangesQuery, is_root_name};
use tempfile::{TempDir, tempdir};

use super::super::{
    baseline_file, changes, read_baseline, remove_baseline, store, worktree, write_baseline,
};
use super::watch::{Event, Kind, Watch};
use super::{
    baseline_object, baseline_root, change_every_kind_of_entry, commit_paths, objects_of, observe,
    paths, remove_store, source_repository, store_of, target, tracked_workspace,
};
use crate::git_baselines::DEFAULT_BYTE_LIMIT;
use crate::{DriverError, DriverErrorClass};

const QUERY: GitChangesQuery = GitChangesQuery {
    max_files: 100,
    max_file_bytes: 1024,
    max_total_bytes: 1024 * 1024,
};

/// What both observations answer, without the instant they were made at.
#[derive(Debug, PartialEq)]
struct Answer {
    commit: String,
    items: Vec<GitChange>,
    truncated: bool,
    returned_bytes: u64,
    file: Option<GitBaselineFile>,
    absent: Option<GitBaselineFile>,
}

/// The changes, a baseline file and a path the baseline does not hold, with `copy_bound` bytes
/// for the copy a store is built from.
fn answer_within(
    workspace: &Path,
    baselines: &Path,
    copy_bound: u64,
) -> Result<Answer, DriverError> {
    target(workspace, baselines, copy_bound, |held| {
        let set = changes(held, QUERY)?;
        let file = baseline_file(held, "README.md", 1024)?;
        let absent = baseline_file(held, "absent.txt", 1024)?;
        Ok(Answer {
            commit: set.commit,
            items: set.items,
            truncated: set.truncated,
            returned_bytes: set.returned_bytes,
            file: file.file,
            absent: absent.file,
        })
    })
}

fn answer(workspace: &Path, baselines: &Path) -> Result<Answer, DriverError> {
    answer_within(workspace, baselines, DEFAULT_BYTE_LIMIT)
}

/// The names in `directory`, sorted; none when it is absent.
fn names(directory: &Path) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
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

/// What the baseline root holds beside the commit-id file and the store of `ws_git`.
fn leftovers(baselines: &Path) -> Vec<String> {
    names(baselines)
        .into_iter()
        .filter(|name| name != "ws_git" && name != ".objects")
        .chain(
            names(&baselines.join(".objects"))
                .into_iter()
                .filter(|name| name != "ws_git")
                .map(|name| format!(".objects/{name}")),
        )
        .collect()
}

fn assert_no_store_and_no_copy(baselines: &Path) {
    assert!(!store_of(baselines).exists(), "a store is kept");
    assert_eq!(
        leftovers(baselines),
        Vec::<String>::new(),
        "something is left in the baseline root"
    );
}

fn mode(path: &Path) -> u32 {
    path.symlink_metadata().expect("entry").permissions().mode() & 0o7777
}

/// The ids of every object the store of `ws_git` holds.
fn store_objects(baselines: &Path) -> BTreeSet<Oid> {
    objects_of(&store_of(baselines))
}

/// The commit `commit` of the repository at `workspace`, every tree it reaches and every file
/// and link object those trees name.
fn baseline_objects(workspace: &Path, commit: Oid) -> BTreeSet<Oid> {
    let repository = Repository::open(workspace).expect("repository");
    let tree = repository
        .find_commit(commit)
        .expect("commit")
        .tree()
        .expect("tree");
    let mut ids = BTreeSet::from([commit, tree.id()]);
    tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
        if entry.kind() != Some(ObjectType::Commit) {
            ids.insert(entry.id());
        }
        git2::TreeWalkResult::Ok
    })
    .expect("tree walk");
    ids
}

/// The commit the commit-id file of `ws_git` names.
fn held_commit(baselines: &Path) -> Oid {
    Oid::from_str(&read_baseline(baselines, "ws_git").expect("commit-id file")).expect("commit id")
}

/// A commit on top of `parent` with the tree `tree`.
fn commit_tree(repository: &Repository, parent: Oid, tree: Oid) -> Oid {
    let signature =
        Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
    let parent = repository.find_commit(parent).expect("parent commit");
    let tree = repository.find_tree(tree).expect("tree");
    repository
        .commit(None, &signature, &signature, "fixture", &tree, &[&parent])
        .expect("commit")
}

/// A pack of the objects of `commit`, written with the Git library alone, as the store of
/// `ws_git`.
fn plant_store(baselines: &Path, workspace: &Path, commit: Oid) {
    let pack = store_of(baselines).join("pack");
    std::fs::create_dir_all(&pack).expect("store");
    let repository = Repository::open(workspace).expect("repository");
    let mut builder = repository.packbuilder().expect("pack builder");
    builder
        .insert_commit(commit)
        .expect("objects of the commit");
    builder.write(&pack, 0).expect("pack and index");
}

/// `length` bytes that no compression makes smaller: a SHA-256 chain of `seed`.
fn incompressible(seed: u8, length: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length + 32);
    let mut block = Sha256::digest([seed]);
    while bytes.len() < length {
        bytes.extend_from_slice(&block);
        block = Sha256::digest(block);
    }
    bytes.truncate(length);
    bytes
}

/// A workspace whose newest commit holds `files` files of `length` incompressible bytes each.
fn noisy_workspace(files: u8, length: usize) -> (TempDir, Oid) {
    let (workspace, first) = source_repository();
    let names: Vec<String> = (0..files).map(|n| format!("noise-{n:02}.bin")).collect();
    for (seed, name) in (0..files).zip(&names) {
        std::fs::write(workspace.path().join(name), incompressible(seed, length))
            .expect("noise file");
    }
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let commit = commit_paths(workspace.path(), first, &names);
    (workspace, commit)
}

/// A workspace materialized before the store existed: its commit-id file, and no store.
fn predating() -> (TempDir, TempDir, Oid) {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    remove_store(baselines.path());
    (workspace, baselines, commit)
}

/// Every entry beneath `directory`, and the bytes of the regular files among them.
fn entries_and_bytes(directory: &Path) -> (usize, u64) {
    let (mut entries, mut bytes) = (0, 0);
    let mut pending = vec![directory.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("directory") {
            let entry = entry.expect("entry");
            let metadata = entry.metadata().expect("metadata");
            entries += 1;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                bytes += metadata.len();
            }
        }
    }
    (entries, bytes)
}

/// A baseline is written store first: the store is made in a staging directory inside the
/// baseline root, of a name no root name can have, and renamed into place, and only then does
/// the commit-id file appear.
#[test]
fn a_baseline_is_written_store_first_through_staging_inside_the_baseline_root() {
    let (workspace, commit) = source_repository();
    let baselines = tempdir().expect("baseline root");
    let stores = baselines.path().join(".objects");
    std::fs::create_dir(&stores).expect("directory of the stores");
    let mut watch = Watch::new();
    watch.add("baselines", baselines.path());
    watch.add("stores", &stores);

    write_baseline(
        baselines.path(),
        "ws_git",
        workspace.path(),
        &commit.to_string(),
        u64::MAX,
    )
    .expect("baseline");

    let events: Vec<Event> = watch
        .events()
        .into_iter()
        .filter(|event| event.kind != Kind::Read)
        .collect();
    let first = |wanted: &dyn Fn(&Event) -> bool, what: &str| {
        events
            .iter()
            .position(wanted)
            .unwrap_or_else(|| panic!("{what}: {events:#?}"))
    };
    let staged = first(
        &|event| {
            event.directory == "baselines"
                && event.kind == Kind::MovedOut
                && !is_root_name(&event.name)
        },
        "the store leaves a staging directory inside the baseline root",
    );
    let installed = first(
        &|event| {
            event.directory == "stores" && event.kind == Kind::MovedIn && event.name == "ws_git"
        },
        "the store is renamed into place",
    );
    let named = first(
        &|event| event.directory == "baselines" && event.name == "ws_git",
        "the commit-id file appears",
    );
    assert!(staged < installed && installed < named, "{events:#?}");
    for event in &events {
        assert!(
            event.directory != "baselines" || event.name == "ws_git" || !is_root_name(&event.name),
            "{event:?} could be a workspace's entry"
        );
    }
    assert_eq!(
        std::fs::read_to_string(baselines.path().join("ws_git")).expect("commit-id file"),
        format!("{commit}\n")
    );
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}

/// The store holds the baseline commit, every tree it reaches and every file and link object
/// those trees name: no history, and no commit of another repository a tree records. It is one
/// pack and its index, in directories of mode 0700 and files of mode 0400.
#[test]
fn the_store_holds_the_commit_its_trees_and_its_file_and_link_objects_and_nothing_else() {
    let (workspace, earlier) = tracked_workspace();
    let parent = held_commit(earlier.path());
    let foreign = Oid::from_str("0123456789012345678901234567890123456789").expect("commit id");
    let commit = {
        let repository = Repository::open(workspace.path()).expect("repository");
        let tree = repository
            .find_commit(parent)
            .expect("parent commit")
            .tree()
            .expect("tree");
        let mut builder = repository.treebuilder(Some(&tree)).expect("tree builder");
        builder
            .insert("vendored", foreign, 0o160_000)
            .expect("another repository's commit");
        let tree = builder.write().expect("tree");
        commit_tree(&repository, parent, tree)
    };
    let baselines = baseline_root(workspace.path(), commit);

    let held = store_objects(baselines.path());
    assert_eq!(held, baseline_objects(workspace.path(), commit));
    // The commit, the root tree and `src`, and eight files and links.
    assert_eq!(held.len(), 11, "{held:#?}");
    assert!(!held.contains(&parent), "the store holds history");
    assert!(!held.contains(&foreign));

    let store = store_of(baselines.path());
    assert_eq!(names(&store), ["pack"]);
    let files = names(&store.join("pack"));
    let [index, pack] = files.as_slice() else {
        panic!("one pack and its index: {files:?}");
    };
    let name = index
        .strip_suffix(".idx")
        .unwrap_or_else(|| panic!("an index: {files:?}"));
    assert_eq!(pack.strip_suffix(".pack"), Some(name), "{files:?}");
    for directory in [
        baselines.path().join(".objects"),
        store.clone(),
        store.join("pack"),
    ] {
        assert_eq!(mode(&directory), 0o700, "{}", directory.display());
    }
    for file in [
        store.join("pack").join(index),
        store.join("pack").join(pack),
    ] {
        assert_eq!(mode(&file), 0o400, "{}", file.display());
    }
    assert_eq!(mode(&baselines.path().join("ws_git")), 0o400);
}

/// A baseline whose tree is empty is held and read, whether or not the repository stores the
/// empty tree.
#[test]
fn a_baseline_whose_tree_is_empty_is_held_and_read() {
    let (workspace, first) = source_repository();
    let commit = {
        let repository = Repository::open(workspace.path()).expect("repository");
        let tree = repository
            .treebuilder(None)
            .expect("tree builder")
            .write()
            .expect("empty tree");
        commit_tree(&repository, first, tree)
    };
    let _ = std::fs::remove_file(
        workspace
            .path()
            .join(".git/objects/4b/825dc642cb6eb9a060e54bf8d69288fbee4904"),
    );
    let baselines = baseline_root(workspace.path(), commit);

    let held = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&held), ["README.md"], "{held:#?}");
    remove_store(baselines.path());
    let built = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&built), ["README.md"], "{built:#?}");
    assert!(store_objects(baselines.path()).contains(&commit));
}

/// Both observations of a workspace that has its store open nothing beneath the workspace's
/// `.git/objects`.
#[test]
fn observations_of_a_workspace_with_a_store_open_nothing_beneath_its_object_store() {
    let (workspace, baselines) = tracked_workspace();
    change_every_kind_of_entry(workspace.path());
    let mut watch = Watch::new();
    let mut pending = vec![workspace.path().join(".git/objects")];
    while let Some(directory) = pending.pop() {
        watch.add("objects", &directory);
        for entry in std::fs::read_dir(&directory).expect("object store") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("kind").is_dir() {
                pending.push(entry.path());
            }
        }
    }
    assert!(
        !watch.events().is_empty(),
        "the watch reports a directory that is opened"
    );

    let answered = answer(workspace.path(), baselines.path()).expect("both observations");
    assert!(answered.file.is_some() && !answered.items.is_empty());
    assert_eq!(watch.events(), Vec::<Event>::new());
}

/// Both observations answer exactly what they answer on the untouched workspace when its
/// `.git/objects` is removed, and when a link, a FIFO or a directory of links stands in its
/// place.
#[test]
fn observations_answer_the_same_whatever_stands_in_place_of_the_workspace_object_store() {
    let (workspace, baselines) = tracked_workspace();
    change_every_kind_of_entry(workspace.path());
    let untouched = answer(workspace.path(), baselines.path()).expect("untouched workspace");
    assert!(untouched.file.is_some() && untouched.items.len() > 10);
    let objects = workspace.path().join(".git/objects");
    let elsewhere = tempdir().expect("directory outside the workspace");
    let outside = elsewhere.path().join("objects");
    std::fs::rename(&objects, &outside).expect("object store moved out");
    let same = |what: &str| {
        let answered = answer(workspace.path(), baselines.path())
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        assert_eq!(answered, untouched, "{what}");
    };

    same("the object store removed");
    std::os::unix::fs::symlink(&outside, &objects).expect("link");
    same("a link in its place");
    std::fs::remove_file(&objects).expect("link removed");
    nix::unistd::mkfifo(&objects, nix::sys::stat::Mode::from_bits_truncate(0o600)).expect("FIFO");
    same("a FIFO in its place");
    std::fs::remove_file(&objects).expect("FIFO removed");
    std::fs::create_dir(&objects).expect("directory");
    for entry in std::fs::read_dir(&outside).expect("object store outside") {
        let entry = entry.expect("entry");
        std::os::unix::fs::symlink(entry.path(), objects.join(entry.file_name())).expect("link");
    }
    same("a directory of links in its place");
}

/// A baseline for a root name that already has a commit-id file or a store is refused before
/// either is written.
#[test]
fn a_baseline_is_refused_for_a_root_name_that_has_a_commit_id_file_or_a_store() {
    let (workspace, commit) = source_repository();
    let write = |baselines: &Path| {
        write_baseline(
            baselines,
            "ws_git",
            workspace.path(),
            &commit.to_string(),
            u64::MAX,
        )
        .expect_err("the root name has an entry")
        .code
    };

    let with_file = tempdir().expect("baseline root");
    std::fs::write(with_file.path().join("ws_git"), b"another workspace's\n").expect("entry");
    assert_eq!(write(with_file.path()), "workspace.git-target-exists");
    assert_eq!(
        std::fs::read(with_file.path().join("ws_git")).expect("entry"),
        b"another workspace's\n"
    );
    assert_no_store_and_no_copy(with_file.path());

    let with_store = tempdir().expect("baseline root");
    let store = store_of(with_store.path());
    std::fs::create_dir_all(&store).expect("store");
    std::fs::write(store.join("marker"), b"another workspace's\n").expect("entry");
    assert_eq!(write(with_store.path()), "workspace.git-target-exists");
    assert!(!with_store.path().join("ws_git").exists());
    assert_eq!(names(&store), ["marker"]);
    assert_eq!(leftovers(with_store.path()), Vec::<String>::new());
}

/// A store over the byte limit is not written: the baseline is refused and nothing is left.
#[test]
fn a_store_over_the_byte_limit_is_refused_and_nothing_is_left() {
    let (workspace, commit) = noisy_workspace(16, 1024);
    let baselines = tempdir().expect("baseline root");
    let write = |max_bytes: u64| {
        write_baseline(
            baselines.path(),
            "ws_git",
            workspace.path(),
            &commit.to_string(),
            max_bytes,
        )
    };

    // Every object is within the limit; the sixteen files together are not.
    let error = write(8 * 1024).expect_err("a store over the limit");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, DriverErrorClass::Exhausted);
    assert!(!baselines.path().join("ws_git").exists());
    assert_no_store_and_no_copy(baselines.path());

    write(64 * 1024).expect("a store within the limit");
    assert_eq!(
        store_objects(baselines.path()),
        baseline_objects(workspace.path(), commit)
    );
}

/// Removing a baseline removes the commit-id file and the store of that root name, and nothing
/// of another.
#[test]
fn removing_a_baseline_removes_its_commit_id_file_and_its_store() {
    let baselines = tempdir().expect("baseline root");
    for name in ["ws_git", "ws_other"] {
        let pack = baselines.path().join(".objects").join(name).join("pack");
        std::fs::create_dir_all(&pack).expect("store");
        std::fs::write(pack.join("pack-0.pack"), b"pack").expect("pack");
        std::fs::write(baselines.path().join(name), b"commit\n").expect("commit-id file");
    }

    remove_baseline(baselines.path(), "ws_git").expect("removal");
    assert!(!baselines.path().join("ws_git").exists());
    assert!(!store_of(baselines.path()).exists());
    assert_eq!(
        leftovers(baselines.path()),
        ["ws_other", ".objects/ws_other"]
    );
    assert!(
        baselines
            .path()
            .join(".objects/ws_other/pack/pack-0.pack")
            .is_file()
    );
    remove_baseline(baselines.path(), "ws_git").expect("removal of a workspace without entries");
}

/// A root name outside the root-name set is refused before an entry is written or removed.
#[test]
fn a_root_name_outside_the_root_name_set_is_refused_before_anything_is_written_or_removed() {
    let (workspace, commit) = source_repository();
    // The baseline root lies two directories deep in a directory of this case's own, so a name
    // that climbs out of it reaches nothing else.
    let outer = tempdir().expect("directory of this case");
    let nest = outer.path().join("nest");
    let baselines = nest.join("baselines");
    std::fs::create_dir_all(&baselines).expect("baseline root");
    std::fs::write(nest.join("ws_git"), b"beside the baseline root\n").expect("file");
    let write = |name: &str| {
        write_baseline(
            &baselines,
            name,
            workspace.path(),
            &commit.to_string(),
            u64::MAX,
        )
    };
    write("ws_git").expect("baseline");
    let state = || {
        (
            names(outer.path()),
            names(&nest),
            names(&baselines),
            names(&baselines.join(".objects")),
            store_objects(&baselines),
        )
    };
    let before = state();
    assert_eq!(before.2, [".objects", "ws_git"]);
    for name in [".objects", "..", ".", "", "a/b", "../ws_git", "-x", "a.b"] {
        let written = write(name).expect_err("a name outside the root-name set");
        assert_eq!(written.code, "workspace.path-escape", "write {name:?}");
        let removed =
            remove_baseline(&baselines, name).expect_err("a name outside the root-name set");
        assert_eq!(removed.code, "workspace.path-escape", "remove {name:?}");
        assert_eq!(state(), before, "{name:?}");
    }
}

/// A workspace with a commit-id file and no store gets its store at the first observation,
/// whichever observation that is. The store holds the baseline's objects, the copy it was made
/// from is gone, and later observations open nothing in the workspace's object store.
#[test]
fn a_workspace_without_a_store_gets_its_store_at_the_first_observation() {
    let (workspace, baselines) = tracked_workspace();
    change_every_kind_of_entry(workspace.path());
    let commit = held_commit(baselines.path());
    let with_store = answer(workspace.path(), baselines.path()).expect("answer with the store");
    let expected = baseline_objects(workspace.path(), commit);

    remove_store(baselines.path());
    let file = target(
        workspace.path(),
        baselines.path(),
        DEFAULT_BYTE_LIMIT,
        |held| baseline_file(held, "README.md", 1024),
    )
    .expect("baseline file of a workspace that predates the store");
    assert_eq!(file.file, with_store.file);
    assert_eq!(store_objects(baselines.path()), expected);
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());

    remove_store(baselines.path());
    let set = target(
        workspace.path(),
        baselines.path(),
        DEFAULT_BYTE_LIMIT,
        |held| changes(held, QUERY),
    )
    .expect("changes of a workspace that predates the store");
    assert_eq!(set.items, with_store.items);
    assert_eq!(store_objects(baselines.path()), expected);
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
    for directory in [
        store_of(baselines.path()),
        store_of(baselines.path()).join("pack"),
    ] {
        assert_eq!(mode(&directory), 0o700, "{}", directory.display());
    }

    std::fs::remove_dir_all(workspace.path().join(".git/objects")).expect("object store removed");
    assert_eq!(
        answer(workspace.path(), baselines.path()).expect("answer from the built store"),
        with_store
    );
}

/// How an object store that holds more than directories and regular files is made from the
/// one at `objects`, whose baseline file object is `object`; `elsewhere` is outside the
/// workspace.
type Tamper = fn(objects: &Path, object: &Path, elsewhere: &Path);

fn object_replaced_by_a_link(_: &Path, object: &Path, elsewhere: &Path) {
    let outside = elsewhere.join("object");
    std::fs::copy(object, &outside).expect("the object's bytes outside the workspace");
    std::fs::remove_file(object).expect("object removed");
    std::os::unix::fs::symlink(&outside, object).expect("link");
}

fn object_replaced_by_a_fifo(_: &Path, object: &Path, _: &Path) {
    std::fs::remove_file(object).expect("object removed");
    nix::unistd::mkfifo(object, nix::sys::stat::Mode::from_bits_truncate(0o600)).expect("FIFO");
}

fn a_socket_beside_the_objects(objects: &Path, _: &Path, _: &Path) {
    drop(std::os::unix::net::UnixListener::bind(objects.join("socket")).expect("socket"));
}

fn an_alternates_file(objects: &Path, _: &Path, elsewhere: &Path) {
    std::fs::create_dir_all(objects.join("info")).expect("directory");
    std::fs::write(
        objects.join("info/alternates"),
        format!("{}\n", elsewhere.display()),
    )
    .expect("object store named by a file");
}

fn a_directory_replaced_by_a_link(_: &Path, object: &Path, elsewhere: &Path) {
    let directory = object.parent().expect("directory of the object");
    let outside = elsewhere.join("directory");
    std::fs::rename(directory, &outside).expect("directory moved out");
    std::os::unix::fs::symlink(&outside, directory).expect("link");
}

fn directories_nested_nine_deep(objects: &Path, _: &Path, _: &Path) {
    std::fs::create_dir_all(objects.join(["d"; 9].join("/"))).expect("nested directories");
}

/// What an observation that has to build a store answers: `Ok` when it built one, else the
/// code of its refusal. `None` when it did not answer within five seconds.
fn build(workspace: &Path, baselines: &Path) -> Option<Result<(), String>> {
    let (answer, answered) = std::sync::mpsc::channel();
    let (root, baseline) = (workspace.to_owned(), baselines.to_owned());
    std::thread::spawn(move || {
        let outcome = observe(&root, &baseline, 1024)
            .map(|_| ())
            .map_err(|error| error.code.to_owned());
        let _ = answer.send(outcome);
    });
    answered.recv_timeout(Duration::from_secs(5)).ok()
}

/// No store is built from an object store that holds a link, a FIFO, another entry that is
/// neither a directory nor a regular file, an `info/alternates` file, or directories nested
/// deeper than eight: the attempt ends `workspace.git-repository-failed` without waiting, and
/// leaves no store and no copy.
#[test]
fn no_store_is_built_from_an_object_store_of_more_than_directories_and_regular_files() {
    let cases: [(&str, Tamper); 6] = [
        ("an object replaced by a link", object_replaced_by_a_link),
        ("an object replaced by a FIFO", object_replaced_by_a_fifo),
        ("a socket", a_socket_beside_the_objects),
        ("info/alternates", an_alternates_file),
        (
            "a directory replaced by a link",
            a_directory_replaced_by_a_link,
        ),
        ("directories nested nine deep", directories_nested_nine_deep),
    ];
    for (what, tamper) in cases {
        let (workspace, baselines, commit) = predating();
        let elsewhere = tempdir().expect("directory outside the workspace");
        let object = baseline_object(workspace.path(), commit);
        tamper(
            &workspace.path().join(".git/objects"),
            &object,
            elsewhere.path(),
        );

        let outcome = build(workspace.path(), baselines.path());
        // Lets a reader that waits on a FIFO return, so the test process keeps no thread.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&object);
        assert_eq!(
            outcome.unwrap_or_else(|| panic!("{what}: the observation waited")),
            Err("workspace.git-repository-failed".to_owned()),
            "{what}"
        );
        assert_no_store_and_no_copy(baselines.path());
    }
}

/// Directories nested eight deep are within the bound: the store is built.
#[test]
fn a_store_is_built_from_an_object_store_with_directories_nested_eight_deep() {
    let (workspace, baselines, commit) = predating();
    std::fs::create_dir_all(
        workspace
            .path()
            .join(".git/objects")
            .join(["d"; 8].join("/")),
    )
    .expect("nested directories");

    observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(
        store_objects(baselines.path()),
        baseline_objects(workspace.path(), commit)
    );
}

/// An object store with more entries than the bound ends the attempt
/// `workspace.git-repository-failed`; one with exactly as many is copied.
#[test]
fn no_store_is_built_from_an_object_store_with_more_entries_than_the_bound() {
    let (workspace, baselines, commit) = predating();
    let (entries, _) = entries_and_bytes(&workspace.path().join(".git/objects"));
    let held = |max_entries: usize| {
        target(
            workspace.path(),
            baselines.path(),
            DEFAULT_BYTE_LIMIT,
            |held| store::held(held, commit, max_entries).map(|_| ()),
        )
    };

    let error = held(entries - 1).expect_err("more entries than the bound");
    assert_eq!(error.code, "workspace.git-repository-failed");
    assert_no_store_and_no_copy(baselines.path());
    held(entries).expect("as many entries as the bound");
    assert!(store_objects(baselines.path()).contains(&commit));
}

/// An object store of more bytes than the copy's bound ends the attempt
/// `workspace.git-storage-limit`; the copy takes exactly the bound and no byte more.
#[test]
fn no_store_is_built_from_an_object_store_of_more_bytes_than_the_bound() {
    let (workspace, baselines, commit) = predating();
    let objects = workspace.path().join(".git/objects");
    let (_, bytes) = entries_and_bytes(&objects);

    let error = answer_within(workspace.path(), baselines.path(), bytes - 1)
        .expect_err("more bytes than the bound");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, DriverErrorClass::Exhausted);
    assert_no_store_and_no_copy(baselines.path());

    let pinned: OwnedFd = std::fs::File::open(&objects).expect("object store").into();
    let copy = |max_bytes: u64| {
        let copy = tempdir().expect("copy");
        worktree::copy_store(&pinned, copy.path(), usize::MAX, max_bytes)
            .map(|()| entries_and_bytes(copy.path()))
    };
    assert_eq!(
        copy(bytes - 1).expect_err("one byte over").code,
        "workspace.git-storage-limit"
    );
    assert_eq!(
        copy(bytes).expect("exactly the bound"),
        entries_and_bytes(&objects)
    );

    answer_within(workspace.path(), baselines.path(), bytes + 64 * 1024)
        .expect("an object store within the bound");
    assert!(store_objects(baselines.path()).contains(&commit));
}

/// An object of the baseline that is absent, or that is not what its id names, ends the attempt
/// `workspace.git-baseline-missing` with no store and no copy left. A later observation tries
/// again.
#[test]
fn an_absent_or_altered_baseline_object_ends_the_build_and_a_later_observation_tries_again() {
    let (workspace, baselines, commit) = predating();
    let object = baseline_object(workspace.path(), commit);
    let bytes = std::fs::read(&object).expect("object");
    let refusal = || {
        observe(workspace.path(), baselines.path(), 1024)
            .expect_err("no store without every object of the baseline")
            .code
    };

    std::fs::remove_file(&object).expect("object removed");
    assert_eq!(refusal(), "workspace.git-baseline-missing");
    assert_no_store_and_no_copy(baselines.path());

    std::fs::write(&object, [0xb7, 0x55, 0x00, 0x00]).expect("other bytes in the object's place");
    assert_eq!(refusal(), "workspace.git-baseline-missing");
    assert_no_store_and_no_copy(baselines.path());

    std::fs::remove_file(&object).expect("other bytes removed");
    std::fs::write(&object, bytes).expect("object restored");
    observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(
        store_objects(baselines.path()),
        baseline_objects(workspace.path(), commit)
    );
}

/// An object of the baseline whose header states a size over the copy's bound is not read: the
/// attempt ends `workspace.git-storage-limit`, though the copy and the object itself are sound.
#[test]
fn a_baseline_object_stated_larger_than_the_bound_is_not_read() {
    let (workspace, first) = source_repository();
    std::fs::write(workspace.path().join("zeros.bin"), vec![0_u8; 1024 * 1024])
        .expect("a file that compresses to almost nothing");
    let commit = commit_paths(workspace.path(), first, &["zeros.bin"]);
    let baselines = baseline_root(workspace.path(), commit);
    remove_store(baselines.path());
    let (_, bytes) = entries_and_bytes(&workspace.path().join(".git/objects"));
    assert!(bytes < 64 * 1024, "the object store is {bytes} bytes");

    let error = answer_within(workspace.path(), baselines.path(), 64 * 1024)
        .expect_err("an object stated larger than the bound");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_no_store_and_no_copy(baselines.path());

    answer_within(workspace.path(), baselines.path(), 2 * 1024 * 1024)
        .expect("the object within the bound");
    assert!(store_objects(baselines.path()).contains(&commit));
}

/// One build runs at a time: an observation that needs one waits for its turn and makes
/// nothing before it. An observation of a workspace that has its store does not wait.
#[test]
fn an_observation_that_needs_a_build_waits_for_its_turn() {
    let (workspace, baselines, commit) = predating();
    let (held_workspace, held_baselines) = tracked_workspace();
    let turn = store::BUILD_TURN.lock();
    let (answer, answered) = std::sync::mpsc::channel();
    let (root, baseline) = (workspace.path().to_owned(), baselines.path().to_owned());
    std::thread::spawn(move || {
        let _ = answer.send(observe(&root, &baseline, 1024).map(|set| set.commit));
    });

    assert!(
        answered.recv_timeout(Duration::from_millis(300)).is_err(),
        "a build ran while another held the turn"
    );
    assert_no_store_and_no_copy(baselines.path());
    let (other, other_answered) = std::sync::mpsc::channel();
    let (root, baseline) = (
        held_workspace.path().to_owned(),
        held_baselines.path().to_owned(),
    );
    std::thread::spawn(move || {
        let _ = other.send(observe(&root, &baseline, 1024).is_ok());
    });
    assert_eq!(
        other_answered.recv_timeout(Duration::from_secs(10)),
        Ok(true),
        "an observation of a workspace with a store waited for a build"
    );

    drop(turn);
    let built = answered
        .recv_timeout(Duration::from_secs(10))
        .expect("the observation answers after its turn")
        .expect("change set");
    assert_eq!(built, commit.to_string());
    assert!(store_objects(baselines.path()).contains(&commit));
}

/// A store that does not hold the commit the commit-id file names is replaced by one that
/// does, built like the store of a workspace that has none.
#[test]
fn a_store_that_lacks_the_named_commit_is_replaced() {
    let (workspace, first) = source_repository();
    std::fs::write(workspace.path().join("later.txt"), b"later\n").expect("file");
    let commit = commit_paths(workspace.path(), first, &["later.txt"]);
    let baselines = baseline_root(workspace.path(), commit);
    std::fs::write(workspace.path().join("README.md"), b"changed workspace\n").expect("change");
    let expected = answer(workspace.path(), baselines.path()).expect("answer with the store");
    remove_store(baselines.path());
    plant_store(baselines.path(), workspace.path(), first);
    assert!(!store_objects(baselines.path()).contains(&commit));

    assert_eq!(
        answer(workspace.path(), baselines.path()).expect("answer"),
        expected
    );
    assert_eq!(
        store_objects(baselines.path()),
        baseline_objects(workspace.path(), commit)
    );
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}
