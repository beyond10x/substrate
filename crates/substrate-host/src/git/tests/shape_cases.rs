//! Cases of the check of a baseline's shape (design 21, § *The baseline store*): the shape of a
//! baseline is checked before anything walks it. The check reads each tree once, however often
//! the baseline names it, and refuses a baseline that nests deeper than the bound or names more
//! paths than the inode limit. A create runs it before it checks anything out, and a build
//! before it writes the store.

use std::path::Path;

use git2::{Oid, Repository};
use substrate_wire::StorageLimit;
use tempfile::{TempDir, tempdir};

use super::super::{finish_materialization, store};
use super::store_cases::{leftovers, store_objects};
use super::store_failure_cases::{
    NESTING_BOUND, both, codes, commit_id_file_alone, nested_repository, repository_of,
    store_of_another_commit,
};
use super::{baseline_file, changes, store_of, target_within};
use crate::DriverErrorClass;
use crate::git_baselines::{DEFAULT_BYTE_LIMIT, DEFAULT_INODE_LIMIT, DEFAULT_LIMIT};

const BRANCH: &str = "refs/heads/provider-default";

/// The tree of a directory that holds one directory twice, as `a` and as `b`, and so on
/// `levels` deep, the innermost empty: `levels + 1` trees that name `2^(levels + 1) - 2` paths.
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
/// directory of [`doubled`]. Its baseline names `2^(levels + 1)` paths in `levels + 2` trees.
pub(in crate::git) fn doubled_repository(levels: u32) -> (TempDir, Oid) {
    repository_of(|repository| vec![("d", doubled(repository, levels))])
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
                    changes(held, super::store_cases::QUERY).map(|_| ()),
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

/// The check reads each tree of the baseline once, however often the baseline names it: a
/// baseline that holds one directory twice at each of forty levels names 2^41 paths in
/// forty-two trees, and the check reads forty-two. Its count does not wrap: seventy levels name
/// more paths than a count holds, and are refused like any number over the limit.
#[test]
fn the_check_reads_a_tree_once_however_often_the_baseline_names_it() {
    let check = |levels: u32, max_inodes: u64| {
        let (workspace, commit) = doubled_repository(levels);
        let repository = Repository::open(workspace.path()).expect("repository");
        store::check_baseline(&repository, commit, inodes(max_inodes)).ok()
    };

    assert_eq!(
        check(40, u64::MAX),
        Some(store::Checked {
            trees: 42,
            paths: 1 << 41,
        })
    );
    assert_eq!(check(40, (1 << 41) - 1), None);
    assert_eq!(
        check(70, u64::MAX),
        Some(store::Checked {
            trees: 72,
            paths: u64::MAX,
        })
    );
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
