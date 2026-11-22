//! Cases of the check of a baseline's shape (design 21, § *The baseline store*): the shape of a
//! baseline is checked before anything walks it. The check reads each tree once, however often
//! the baseline names it, and holds the bounds of nesting, of names and path lengths, of object
//! sizes, of paths and of bytes. A create runs it before it checks anything out, and a build
//! before it writes the store.

use std::path::Path;

use git2::{ObjectType, Oid, Repository};
use substrate_wire::StorageLimit;
use tempfile::{TempDir, tempdir};

use super::super::{finish_materialization, store};
use super::store_cases::{QUERY, leftovers, store_objects};
use super::store_failure_cases::{
    NESTING_BOUND, both, codes, commit_id_file_alone, nested_repository, repository_holding,
    repository_of, store_of_another_commit,
};
use super::{baseline_file, changes, store_of, target_within};
use crate::DriverErrorClass;
use crate::git_baselines::{DEFAULT_BYTE_LIMIT, DEFAULT_INODE_LIMIT, DEFAULT_LIMIT};

const BRANCH: &str = "refs/heads/provider-default";

/// The tree of a directory that holds one directory twice, as `a` and as `b`, and so on
/// `levels` deep, the innermost empty: a directory that names `2^(levels + 1) - 2` paths with
/// `levels + 1` trees.
fn doubled(repository: &Repository, levels: u32) -> Oid {
    let mut innermost = repository
        .treebuilder(None)
        .and_then(|builder| builder.write())
        .expect("empty directory");
    for _ in 0..levels {
        let mut builder = repository.treebuilder(None).expect("tree builder");
        for name in ["a", "b"] {
            builder
                .insert(name, innermost, 0o040_000)
                .expect("directory");
        }
        innermost = builder.write().expect("directory");
    }
    innermost
}

/// A repository whose branch `provider-default` holds one commit: a file, and as `d` the
/// directory of [`doubled`]: a baseline that names `2^(levels + 1)` paths with `levels + 2` trees.
pub(in crate::git) fn doubled_repository(levels: u32) -> (TempDir, Oid) {
    repository_of(|repository| vec![("d".to_owned(), doubled(repository, levels))])
}

/// The limit of a create that requests `max_inodes` inodes and no particular byte limit.
fn inodes(max_inodes: u64) -> StorageLimit {
    StorageLimit {
        max_bytes: DEFAULT_BYTE_LIMIT,
        max_inodes,
    }
}

/// What the checkout step of a create left in the repository's directory beside `.git`, and
/// whether it wrote an index.
fn checked_out(workspace: &Path) -> (Vec<String>, bool) {
    let mut names: Vec<String> = std::fs::read_dir(workspace)
        .expect("repository directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != ".git")
        .collect();
    names.sort();
    (names, workspace.join(".git/index").exists())
}

const NOTHING: (Vec<String>, bool) = (Vec::new(), false);

/// A create refuses a baseline that names more paths than its inode limit
/// `workspace.git-storage-limit`, and checks nothing out: the baseline here names 262,144
/// paths in nineteen trees, against the 200,000 of a create that requests no limit.
#[test]
fn a_create_refuses_a_baseline_that_names_more_paths_than_its_inode_limit_and_checks_nothing_out() {
    let (workspace, commit) = doubled_repository(17);
    let repository = Repository::open(workspace.path()).expect("repository");

    let error = finish_materialization(&repository, BRANCH, commit, DEFAULT_LIMIT)
        .expect_err("more paths than the inode limit");

    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, DriverErrorClass::Exhausted);
    assert_eq!(checked_out(workspace.path()), NOTHING);
    assert!(!repository.head_detached().expect("HEAD"));
}

/// A baseline that names exactly as many paths as the inode limit is served, and one that
/// names one more is refused, by the checkout step of a create and by the write of its store.
/// Every entry counts once for each place the baseline names it, a directory that holds nothing
/// included: this baseline names a file and fifteen directories in five trees.
#[test]
fn a_baseline_that_names_as_many_paths_as_the_inode_limit_is_served_and_one_more_is_refused() {
    let write = |limit: StorageLimit| {
        let (workspace, commit) = doubled_repository(3);
        let repository = Repository::open(workspace.path()).expect("repository");
        let checkout = finish_materialization(&repository, BRANCH, commit, limit);
        let baselines = tempdir().expect("baseline root");
        let written = super::super::write_baseline(
            baselines.path(),
            "ws_git",
            workspace.path(),
            &commit.to_string(),
            limit,
        );
        let left = std::fs::read_dir(baselines.path())
            .expect("baseline root")
            .count();
        let outcome =
            [checkout, written].map(|step| step.map_or_else(|error| error.code, |()| "served"));
        (outcome, checked_out(workspace.path()), left)
    };

    let (outcome, checkout, left) = write(inodes(16));
    assert_eq!(outcome, ["served"; 2]);
    assert_eq!(checkout, (vec!["README.md".to_owned()], true));
    assert_eq!(
        left, 2,
        "the commit-id file and the directory of the stores"
    );

    let (outcome, checkout, left) = write(inodes(15));
    assert_eq!(outcome, ["workspace.git-storage-limit"; 2]);
    assert_eq!(checkout, NOTHING);
    assert_eq!(left, 0, "the refused write left an entry");
}

/// A build refuses a baseline that names more paths than the inode limit
/// `workspace.git-storage-limit`, for both observations, and makes no store and leaves no copy.
/// The limit is 200,000 for a workspace under no project quota.
#[test]
fn a_build_refuses_a_baseline_that_names_more_paths_than_the_inode_limit() {
    let (workspace, commit) = doubled_repository(17);
    let baselines = commit_id_file_alone(commit);

    assert_eq!(
        codes(both(workspace.path(), baselines.path())),
        ["workspace.git-storage-limit"; 2]
    );
    assert!(!store_of(baselines.path()).exists(), "a store is kept");
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}

/// A build that is refused for the paths its baseline names leaves the store that was there.
/// The baseline here names two paths; with a limit of two the store is built, and what the
/// build read is in it.
#[test]
fn a_build_refused_for_the_paths_of_its_baseline_leaves_the_store_that_was_there() {
    let (workspace, baselines, commit) = store_of_another_commit();
    let before = store_objects(baselines.path());
    let observe = |max_inodes: u64| {
        target_within(
            workspace.path(),
            baselines.path(),
            inodes(max_inodes),
            |held| {
                [
                    changes(held, QUERY).map(|_| ()),
                    baseline_file(held, "README.md", 1024).map(|_| ()),
                ]
            },
        )
    };

    assert_eq!(codes(observe(1)), ["workspace.git-storage-limit"; 2]);
    assert_eq!(store_objects(baselines.path()), before);
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());

    assert_eq!(codes(observe(2)), ["answered"; 2]);
    assert!(store_objects(baselines.path()).contains(&commit));
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}

/// The check reads each tree of the baseline once, however often the baseline names it: of a
/// baseline that names 2^41 paths with forty-two trees it reads forty-two. Its counts
/// saturate: a baseline of seventy-two trees that names more paths than a count holds is
/// refused like any number over the limit.
#[test]
fn the_check_reads_a_tree_once_however_often_the_baseline_names_it() {
    let check = |levels: u32, max_inodes: u64| {
        let (workspace, commit) = doubled_repository(levels);
        let repository = Repository::open(workspace.path()).expect("repository");
        store::check_baseline(&repository, commit, inodes(max_inodes))
            .ok()
            .map(|checked| (checked.trees, checked.paths))
    };

    assert_eq!(check(40, u64::MAX), Some((42, 1 << 41)));
    assert_eq!(check(40, (1 << 41) - 1), None);
    assert_eq!(check(70, u64::MAX), Some((72, u64::MAX)));
    assert_eq!(check(70, DEFAULT_INODE_LIMIT), None);
}

/// A create refuses a baseline nested deeper than the bound
/// `workspace.git-path-unrepresentable` before it checks anything out; one nested exactly as
/// deep as the bound is checked out.
#[test]
fn a_create_refuses_a_baseline_nested_deeper_than_the_bound_before_it_checks_anything_out() {
    let checkout = |levels: usize| {
        let (workspace, commit) = nested_repository(levels);
        let repository = Repository::open(workspace.path()).expect("repository");
        let outcome = finish_materialization(&repository, BRANCH, commit, DEFAULT_LIMIT)
            .map_or_else(|error| error.code, |()| "served");
        (outcome, checked_out(workspace.path()))
    };

    assert_eq!(
        checkout(NESTING_BOUND + 1),
        ("workspace.git-path-unrepresentable", NOTHING)
    );
    assert_eq!(
        checkout(NESTING_BOUND),
        ("served", (vec!["README.md".to_owned()], true))
    );
}

/// A name of `bytes` bytes.
fn name(bytes: usize) -> String {
    "n".repeat(bytes)
}

/// Directories nested in each other, the innermost empty, each named by as many bytes as
/// `names` says, from the outermost in. Answers the outermost, with its name.
fn named(repository: &Repository, names: &[usize]) -> (String, Oid) {
    let mut held = repository
        .treebuilder(None)
        .and_then(|builder| builder.write())
        .expect("empty directory");
    for bytes in names[1..].iter().rev() {
        let mut builder = repository.treebuilder(None).expect("tree builder");
        builder
            .insert(name(*bytes), held, 0o040_000)
            .expect("directory");
        held = builder.write().expect("directory");
    }
    (name(names[0]), held)
}

/// A repository whose baseline names `README.md` and the directories of [`named`]: one path
/// for each of `names`, the longest of as many bytes as the names and their separators.
fn named_repository(names: &[usize]) -> (TempDir, Oid) {
    repository_of(|repository| vec![named(repository, names)])
}

/// The lengths of names that make one path of `bytes` bytes, separators included.
fn names_of_a_path(bytes: usize) -> Vec<usize> {
    let mut names = Vec::new();
    let mut left = bytes;
    while left > store::MAX_NAME_BYTES {
        // A name, its separator, and at least one byte of the next name.
        let name = store::MAX_NAME_BYTES.min(left - 2);
        names.push(name);
        left -= name + 1;
    }
    names.push(left);
    assert_eq!(names.iter().sum::<usize>() + names.len() - 1, bytes);
    names
}

/// What the checkout step of a create answers for the baseline of the repository at
/// `workspace`, and what it left there.
fn checkout(
    workspace: &Path,
    commit: Oid,
    limit: StorageLimit,
) -> (&'static str, (Vec<String>, bool)) {
    let repository = Repository::open(workspace).expect("repository");
    let outcome = finish_materialization(&repository, BRANCH, commit, limit)
        .map_or_else(|error| error.code, |()| "served");
    (outcome, checked_out(workspace))
}

/// What a checkout step that served a baseline of `README.md` and empty directories left.
fn readme_alone() -> (Vec<String>, bool) {
    (vec!["README.md".to_owned()], true)
}

/// What the check found in the baseline of the repository at `workspace`.
fn checked(workspace: &Path, commit: Oid, limit: StorageLimit) -> Option<store::Checked> {
    let repository = Repository::open(workspace).expect("repository");
    store::check_baseline(&repository, commit, limit).ok()
}

/// A create refuses a baseline that names a path no checkout can make
/// `workspace.git-path-unrepresentable`, before it checks anything out. The baseline here
/// names five paths with five trees, four of them by names of 60,000 bytes.
#[test]
fn a_create_refuses_a_baseline_that_names_a_path_no_checkout_can_make_before_it_checks_anything_out()
 {
    let (workspace, commit) = named_repository(&[60_000; 4]);

    assert_eq!(
        checkout(workspace.path(), commit, DEFAULT_LIMIT),
        ("workspace.git-path-unrepresentable", NOTHING)
    );
}

/// A name of 255 bytes is within the bound, and one of 256 bytes is refused
/// `workspace.git-path-unrepresentable` before anything is checked out.
#[test]
fn a_name_of_255_bytes_is_served_and_one_of_256_bytes_is_refused() {
    assert_eq!(store::MAX_NAME_BYTES, 255);
    let (workspace, commit) = named_repository(&[255]);
    assert_eq!(
        checked(workspace.path(), commit, DEFAULT_LIMIT).map(|checked| checked.longest),
        Some(255)
    );
    assert_eq!(
        checkout(workspace.path(), commit, DEFAULT_LIMIT),
        ("served", readme_alone())
    );

    let (workspace, commit) = named_repository(&[256]);
    assert_eq!(
        checkout(workspace.path(), commit, DEFAULT_LIMIT),
        ("workspace.git-path-unrepresentable", NOTHING)
    );
}

/// A path of 4,096 bytes is within the bound, and one of 4,097 bytes is refused
/// `workspace.git-path-unrepresentable` before anything is checked out. Each path here is
/// seventeen directories nested in each other, none named by more than 255 bytes.
#[test]
fn a_path_of_4096_bytes_is_served_and_one_of_4097_bytes_is_refused() {
    assert_eq!(store::MAX_PATH_BYTES, 4096);
    let (workspace, commit) = named_repository(&names_of_a_path(4096));
    let found = checked(workspace.path(), commit, DEFAULT_LIMIT).expect("within the bounds");
    assert_eq!((found.paths, found.longest), (18, 4096));
    assert_eq!(
        checkout(workspace.path(), commit, DEFAULT_LIMIT),
        ("served", readme_alone())
    );

    let (workspace, commit) = named_repository(&names_of_a_path(4097));
    assert_eq!(
        checkout(workspace.path(), commit, DEFAULT_LIMIT),
        ("workspace.git-path-unrepresentable", NOTHING)
    );
}

/// The paths of a baseline together take at most 256 bytes for each path the inode limit
/// admits: with a limit of four paths, a baseline whose four paths are 1,024 bytes together is
/// served, and one whose four paths are 1,025 bytes is refused `workspace.git-storage-limit`
/// before anything is checked out.
#[test]
fn the_paths_of_a_baseline_together_are_bounded_by_256_bytes_for_each_admitted_path() {
    assert_eq!(store::PATH_BYTES_PER_PATH, 256);
    // `README.md` and three directories nested in each other: 9 + 255 + 356 + 404 bytes.
    let (workspace, commit) = named_repository(&[255, 100, 47]);
    let found = checked(workspace.path(), commit, inodes(4)).expect("within the bounds");
    assert_eq!((found.paths, found.path_bytes), (4, 1024));
    assert_eq!(
        checkout(workspace.path(), commit, inodes(4)),
        ("served", readme_alone())
    );

    let (workspace, commit) = named_repository(&[255, 100, 48]);
    assert_eq!(
        checkout(workspace.path(), commit, inodes(4)),
        ("workspace.git-storage-limit", NOTHING)
    );
}

/// The loose object file of `id` in the repository at `workspace`.
fn loose(workspace: &Path, id: Oid) -> std::path::PathBuf {
    let id = id.to_string();
    workspace.join(".git/objects").join(&id[..2]).join(&id[2..])
}

/// Puts the bytes of the object `other` into the place of the object `id`: what stands there
/// then states the type and the size of `other`, and is not what `id` names, so reading it
/// whole fails its check.
fn stand_in(workspace: &Path, id: Oid, other: Oid) {
    std::fs::remove_file(loose(workspace, id)).expect("object removed");
    std::fs::copy(loose(workspace, other), loose(workspace, id)).expect("another object's bytes");
}

/// The commit's header is compared with the byte limit before the commit is read: a commit
/// that states more is refused `workspace.git-storage-limit`. What stands in the commit's
/// place here states 8 KiB and is not the commit, so with a limit above that the commit is
/// read and found missing.
#[test]
fn a_commit_that_states_more_than_the_byte_limit_is_refused_before_it_is_read() {
    let (workspace, commit) = repository_of(|_| Vec::new());
    {
        let repository = Repository::open(workspace.path()).expect("repository");
        let tree = repository
            .find_commit(commit)
            .and_then(|commit| commit.tree())
            .expect("tree");
        let signature =
            git2::Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
        let larger = repository
            .commit(None, &signature, &signature, &"m".repeat(8192), &tree, &[])
            .expect("a larger commit");
        stand_in(workspace.path(), commit, larger);
    }
    let bytes = |max_bytes: u64| StorageLimit {
        max_bytes,
        max_inodes: DEFAULT_INODE_LIMIT,
    };

    assert_eq!(
        checkout(workspace.path(), commit, bytes(8192)),
        ("workspace.git-storage-limit", NOTHING)
    );
    assert_eq!(
        checkout(workspace.path(), commit, bytes(16 * 1024)),
        ("workspace.git-commit-missing", NOTHING)
    );
}

/// The tree of a directory that holds `files` files, each by a name of 255 bytes: a tree of
/// 283 bytes for each.
fn tree_of_files(repository: &Repository, files: u8) -> Oid {
    let file = repository.blob(b"").expect("file");
    let mut builder = repository.treebuilder(None).expect("tree builder");
    for letter in (b'a'..).take(usize::from(files)) {
        builder
            .insert(char::from(letter).to_string().repeat(255), file, 0o100_644)
            .expect("file");
    }
    builder.write().expect("directory")
}

/// A tree's header is compared with 283 bytes for each path the inode limit admits before the
/// tree is read. A tree of exactly that size is read: two files with names of 255 bytes under a
/// limit of two paths. One that states more is refused `workspace.git-storage-limit`: what
/// stands in the tree's place here states 849 bytes and is not the tree, so with a limit of
/// three paths it is read and fails its check.
#[test]
fn a_tree_that_states_more_than_283_bytes_for_each_admitted_path_is_refused_before_it_is_read() {
    assert_eq!(store::TREE_BYTES_PER_PATH, 283);
    let (workspace, commit) = repository_holding(|repository| tree_of_files(repository, 2));
    assert_eq!(
        checked(workspace.path(), commit, inodes(2)).map(|found| (found.trees, found.paths)),
        Some((1, 2))
    );

    let mut tree = None;
    let (workspace, commit) = repository_holding(|repository| {
        let root = tree_of_files(repository, 1);
        tree = Some((root, tree_of_files(repository, 3)));
        root
    });
    let (root, larger) = tree.expect("trees");
    stand_in(workspace.path(), root, larger);

    assert_eq!(
        checkout(workspace.path(), commit, inodes(2)),
        ("workspace.git-storage-limit", NOTHING)
    );
    assert_eq!(
        checkout(workspace.path(), commit, inodes(3)),
        ("workspace.git-checkout-failed", NOTHING)
    );
}

/// One entry of a tree, as the bytes a tree object holds for it.
fn entry(mode: &str, name: &str, id: Oid) -> Vec<u8> {
    let mut bytes = format!("{mode} {name}\0").into_bytes();
    bytes.extend(id.as_bytes());
    bytes
}

/// Writes a tree object of exactly `bytes` into the repository.
fn tree_of(repository: &Repository, bytes: &[u8]) -> Oid {
    repository
        .odb()
        .and_then(|objects| objects.write(ObjectType::Tree, bytes))
        .expect("tree object")
}

/// The limit of a create that requests `max_bytes` bytes and no particular inode limit.
fn bytes(max_bytes: u64) -> StorageLimit {
    StorageLimit {
        max_bytes,
        max_inodes: DEFAULT_INODE_LIMIT,
    }
}

/// A tree states at most 283 bytes for each entry it holds, and one that states more is refused
/// `workspace.git-storage-limit` before anything is checked out. Each of the eight trees here
/// holds one entry and states 250,028 bytes: the mode of its entry is written with 250,000
/// leading zeros. Under a limit of 1,000 paths each is within the 283,000 bytes a tree's header
/// may state, so each is read.
#[test]
fn a_tree_takes_no_more_bytes_than_283_for_each_entry_it_holds() {
    let padding = "0".repeat(250_000);
    let (workspace, commit) = repository_holding(|repository| {
        let file = repository.blob(b"bounded git workspace\n").expect("file");
        let mut held = tree_of(repository, &entry(&format!("{padding}100644"), "f", file));
        for _ in 0..7 {
            held = tree_of(repository, &entry(&format!("{padding}40000"), "d", held));
        }
        held
    });

    assert_eq!(checked(workspace.path(), commit, inodes(1000)), None);
    assert_eq!(
        checkout(workspace.path(), commit, inodes(1000)),
        ("workspace.git-storage-limit", NOTHING)
    );
}

/// A baseline is refused `workspace.git-storage-limit` as soon as the trees the check has read
/// hold more entries than the inode limit: it reads no further tree. The root here names three
/// directories, and none of the three is in the repository. With a limit of three paths the
/// root is within the count, a directory is read, and its absence ends the create
/// `workspace.git-checkout-failed`; with a limit of two the root alone ends it.
#[test]
fn a_baseline_over_the_inode_limit_is_refused_before_another_of_its_trees_is_read() {
    let (workspace, commit) = repository_holding(|repository| {
        let mut root = Vec::new();
        for (name, byte) in [("a", 0x11), ("b", 0x22), ("c", 0x33)] {
            let absent = Oid::from_bytes(&[byte; 20]).expect("an id");
            root.extend(entry("40000", name, absent));
        }
        tree_of(repository, &root)
    });

    assert_eq!(
        checkout(workspace.path(), commit, inodes(3)),
        ("workspace.git-checkout-failed", NOTHING)
    );
    assert_eq!(
        checkout(workspace.path(), commit, inodes(2)),
        ("workspace.git-storage-limit", NOTHING)
    );
}

/// The check keeps no more of a baseline than its inode limit admits. Each of the 32 trees
/// here names the next one 200 times, the innermost an empty directory. Under a limit of 20
/// paths the check ends at the first tree it reads, the root, and keeps the id of none of its
/// directories. Under a limit of 6,400, the entries the 33 trees hold together, it reads every
/// tree once and then refuses the baseline for the paths it names, keeping the ids of fewer
/// directories it has yet to enter than the limit.
#[test]
fn the_check_keeps_no_more_of_a_baseline_than_its_inode_limit_admits() {
    let (workspace, commit) = repository_holding(|repository| {
        let mut held = repository
            .treebuilder(None)
            .and_then(|builder| builder.write())
            .expect("empty directory");
        for _ in 0..32 {
            held = tree_of(repository, &entry("40000", "d", held).repeat(200));
        }
        held
    });
    let repository = Repository::open(workspace.path()).expect("repository");
    let check = |max_inodes: u64| {
        let (checked, kept) =
            store::check_baseline_keeping(&repository, commit, inodes(max_inodes));
        (checked.is_ok(), kept)
    };

    assert_eq!(
        check(20),
        (
            false,
            store::Kept {
                trees: 1,
                entries: 200,
                waiting: 0
            }
        )
    );
    let (admitted, kept) = check(6400);
    assert!(!admitted, "200^32 paths under a limit of 6,400");
    assert_eq!((kept.trees, kept.entries), (33, 6400));
    assert!(kept.waiting < 6400, "{kept:?}");
    assert_eq!(check(6399).1.trees, 32);
}

/// A name that holds a `/` is refused `workspace.git-path-unrepresentable` before anything is
/// checked out: no checkout can make it as one entry. The baseline here holds `README.md` and
/// a file whose name holds five separators.
#[test]
fn a_name_that_holds_a_separator_is_refused_before_anything_is_checked_out() {
    let (workspace, commit) = repository_holding(|repository| {
        let file = repository.blob(b"bounded git workspace\n").expect("file");
        let mut root = entry("100644", "README.md", file);
        root.extend(entry("100644", "a/b/c/d/e/f", file));
        tree_of(repository, &root)
    });

    assert_eq!(checked(workspace.path(), commit, DEFAULT_LIMIT), None);
    assert_eq!(
        checkout(workspace.path(), commit, DEFAULT_LIMIT),
        ("workspace.git-path-unrepresentable", NOTHING)
    );
}

/// How many entries lie beneath `directory`, directories included.
fn entries_beneath(directory: &Path) -> u64 {
    let mut found = 0;
    for entry in std::fs::read_dir(directory).expect("directory") {
        let entry = entry.expect("entry");
        found += 1;
        if entry.file_type().expect("entry type").is_dir() {
            found += entries_beneath(&entry.path());
        }
    }
    found
}

/// A checkout writes nothing through a link its baseline names. Each baseline here names a
/// link `l` to a directory outside the workspace and a file beneath `l`: in the first as an
/// entry whose name holds a separator, in the second as a directory that is also named `l`.
/// Whatever the checkout answers, the directory outside stays empty.
#[test]
fn a_checkout_writes_nothing_through_a_link_its_baseline_names() {
    for doubled in [false, true] {
        let outside = tempdir().expect("a directory outside the workspace");
        let target = outside.path().to_str().expect("path").to_owned();
        let (workspace, commit) = repository_holding(|repository| {
            let file = repository.blob(b"through the link\n").expect("file");
            let link = repository.blob(target.as_bytes()).expect("link target");
            let mut root = entry("120000", "l", link);
            if doubled {
                let held = tree_of(repository, &entry("100644", "x", file));
                root.extend(entry("40000", "l", held));
            } else {
                root.extend(entry("100644", "l/x", file));
            }
            tree_of(repository, &root)
        });

        let (outcome, _) = checkout(workspace.path(), commit, DEFAULT_LIMIT);

        assert_eq!(
            entries_beneath(outside.path()),
            0,
            "the checkout answered {outcome} and wrote outside the workspace (doubled: {doubled})"
        );
    }
}

/// A file object of `length` bytes, each of them `fill`.
fn file_of(repository: &Repository, fill: u8, length: usize) -> Oid {
    repository.blob(&vec![fill; length]).expect("file")
}

/// The files and links of a baseline together state at most the byte limit: three files of
/// 1,000 bytes and a link whose target is 96 bytes long are served under a limit of 3,096
/// bytes and checked out, and refused `workspace.git-storage-limit` under one of 3,095, before
/// anything is checked out. No object here is over either limit.
#[test]
fn the_files_of_a_baseline_together_state_no_more_than_the_byte_limit() {
    let (workspace, commit) = repository_holding(|repository| {
        let mut root = repository.treebuilder(None).expect("tree builder");
        for (name, fill) in [("a", b'a'), ("b", b'b'), ("c", b'c')] {
            root.insert(name, file_of(repository, fill, 1000), 0o100_644)
                .expect("file");
        }
        let target = repository.blob("t".repeat(96).as_bytes()).expect("target");
        root.insert("l", target, 0o120_000).expect("link");
        root.write().expect("root tree")
    });
    assert_eq!(
        checked(workspace.path(), commit, bytes(3096)).map(|found| (found.paths, found.bytes)),
        Some((4, 3096))
    );

    assert_eq!(
        checkout(workspace.path(), commit, bytes(3095)),
        ("workspace.git-storage-limit", NOTHING)
    );
    let names = ["a", "b", "c", "l"].map(str::to_owned).to_vec();
    assert_eq!(
        checkout(workspace.path(), commit, bytes(3096)),
        ("served", (names, true))
    );
}

/// A file counts toward the byte limit once for each place the baseline names it. The one
/// file object here, of 1,024 bytes, is named in five places: three times at the top, and once
/// in a directory the baseline holds twice. That is 5,120 bytes: served under a limit of 5,120
/// and refused `workspace.git-storage-limit` under one of 5,119, before anything is checked
/// out.
#[test]
fn a_file_counts_toward_the_byte_limit_once_for_each_place_the_baseline_names_it() {
    let (workspace, commit) = repository_holding(|repository| {
        let file = file_of(repository, b'f', 1024);
        let mut directory = repository.treebuilder(None).expect("tree builder");
        directory.insert("f", file, 0o100_644).expect("file");
        let directory = directory.write().expect("directory");
        let mut root = repository.treebuilder(None).expect("tree builder");
        for name in ["a", "b", "c"] {
            root.insert(name, file, 0o100_644).expect("file");
        }
        for name in ["d", "e"] {
            root.insert(name, directory, 0o040_000).expect("directory");
        }
        root.write().expect("root tree")
    });
    assert_eq!(
        checked(workspace.path(), commit, bytes(5120)).map(|found| (found.paths, found.bytes)),
        Some((7, 5120))
    );

    assert_eq!(
        checkout(workspace.path(), commit, bytes(5119)),
        ("workspace.git-storage-limit", NOTHING)
    );
    let names = ["a", "b", "c", "d", "e"].map(str::to_owned).to_vec();
    assert_eq!(
        checkout(workspace.path(), commit, bytes(5120)),
        ("served", (names, true))
    );
}

/// A build refuses a commit whose header states more than the byte limit
/// `workspace.git-storage-limit`, for both observations. The commit here states more than
/// 8 KiB, with a message of 8,192 bytes; under a limit of 16 KiB its store is built.
#[test]
fn a_build_refuses_a_commit_that_states_more_than_the_byte_limit() {
    let (workspace, commit) = repository_of(|_| Vec::new());
    let larger = {
        let repository = Repository::open(workspace.path()).expect("repository");
        let tree = repository
            .find_commit(commit)
            .and_then(|commit| commit.tree())
            .expect("tree");
        let signature =
            git2::Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
        repository
            .commit(None, &signature, &signature, &"m".repeat(8192), &tree, &[])
            .expect("a larger commit")
    };
    let observe = |max_bytes: u64| {
        let baselines = commit_id_file_alone(larger);
        let outcome = target_within(
            workspace.path(),
            baselines.path(),
            bytes(max_bytes),
            |held| {
                [
                    changes(held, QUERY).map(|_| ()),
                    baseline_file(held, "README.md", 1024).map(|_| ()),
                ]
            },
        );
        (codes(outcome), store_of(baselines.path()).exists())
    };

    assert_eq!(observe(8192), (["workspace.git-storage-limit"; 2], false));
    assert_eq!(observe(16 * 1024), (["answered"; 2], true));
}
