//! Cases of the baseline store (design 21, § *The baseline store*) through the production
//! driver and the real TLS/upload-pack peer.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_wire::{
    GitBaselineFile, GitChange, GitChangesQuery, GitSourceEnvelope, Workspace,
    WorkspaceCreateInput, WorkspaceSource,
};
use tempfile::tempdir;

use super::{AUTHORITY, Mode, Server, repository};
use crate::git::tests::objects_of;
use crate::git::tests::shape_cases::doubled_repository;
use crate::git::tests::store_failure_cases::{
    NESTING_BOUND, nested_repository, repository_holding,
};
use crate::git::tests::watch::{Event, Kind, Watch};
use crate::{
    DispatchOutcome, Driver as _, DriverError, HostConfig, HostDriver, WorkspaceDestroyProgress,
};

pub(super) fn input(server: &Server, commit: git2::Oid) -> WorkspaceCreateInput {
    WorkspaceCreateInput {
        source: WorkspaceSource::Git(GitSourceEnvelope {
            git: server.source(commit),
        }),
        labels: BTreeMap::default(),
        storage: None,
        lease_ttl_ms: None,
    }
}

pub(super) fn driver(root: &Path, server: &Server) -> Arc<HostDriver> {
    let mut config = HostConfig::minimum(root);
    config.git_sources.push(server.binding.clone());
    HostDriver::open(config).expect("host driver")
}

pub(super) fn baselines(root: &Path) -> PathBuf {
    root.join(".substrate-git-baselines")
}

/// The names in `directory`, sorted; none when it is absent.
pub(super) fn names(directory: &Path) -> Vec<String> {
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

/// The staging trees of Git creates in the directory of all workspaces.
pub(super) fn staging_paths(root: &Path) -> Vec<PathBuf> {
    names(root)
        .into_iter()
        .filter(|name| name.starts_with(".substrate-git-") && name != ".substrate-git-baselines")
        .map(|name| root.join(name))
        .collect()
}

/// How a create that was not served ended, and its refusal.
pub(super) fn refused(outcome: DispatchOutcome<Workspace>) -> (&'static str, DriverError) {
    match outcome {
        DispatchOutcome::Observed(_) => panic!("the create was served"),
        DispatchOutcome::NotDispatched(error) => ("not dispatched", error),
        DispatchOutcome::ContainedAbsent(error) => ("contained absent", error),
        DispatchOutcome::OutcomeUnknown(error) => ("outcome unknown", error),
    }
}

const QUERY: GitChangesQuery = GitChangesQuery {
    max_files: 10,
    max_file_bytes: 1024,
    max_total_bytes: 4096,
};

/// What both observations of `ws_a` answer, without the instant they were made at.
async fn observe(driver: &HostDriver) -> (Vec<GitChange>, Option<GitBaselineFile>) {
    let changes = driver
        .read_workspace_git_changes_v2("ws_a", "ws_a", &QUERY)
        .await
        .expect("change set");
    let file = driver
        .read_workspace_git_baseline_file_v2("ws_a", "ws_a", "README.md", 1024)
        .await
        .expect("baseline file");
    (changes.items, file.file)
}

/// A create writes the store, then the commit-id file, then installs the workspace. The store
/// holds the commit, its tree and its file and nothing of the fetched history, and both
/// observations read it: they answer the same without the workspace's object store.
#[tokio::test]
async fn a_create_writes_the_store_then_the_commit_id_file_then_installs_the_workspace() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    let baselines = baselines(root.path());
    let stores = baselines.join(".objects");
    std::fs::create_dir(&stores).expect("directory of the stores");
    std::fs::set_permissions(&stores, std::fs::Permissions::from_mode(0o700)).expect("mode");
    let mut watch = Watch::new();
    watch.add("workspaces", root.path());
    watch.add("baselines", &baselines);
    watch.add("stores", &stores);

    let created = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;
    assert!(matches!(created, DispatchOutcome::Observed(_)));

    let events: Vec<Event> = watch
        .events()
        .into_iter()
        .filter(|event| event.kind != Kind::Read && event.name == "ws_a")
        .collect();
    let order: Vec<&str> = events.iter().map(|event| event.directory).collect();
    assert_eq!(order, ["stores", "baselines", "workspaces"], "{events:#?}");

    let workspace = root.path().join("ws_a");
    let expected = {
        let repository = git2::Repository::open(&workspace).expect("workspace repository");
        let mut history = repository.revwalk().expect("history");
        history.push_head().expect("history start");
        assert_eq!(
            history.count(),
            50,
            "the workspace holds the fetched history"
        );
        let tree = repository
            .find_commit(commit)
            .expect("commit")
            .tree()
            .expect("tree");
        let file = tree.get_name("README.md").expect("file").id();
        BTreeSet::from([commit, tree.id(), file])
    };
    assert_eq!(objects_of(&stores.join("ws_a")), expected);
    assert_eq!(
        std::fs::read_to_string(baselines.join("ws_a")).expect("commit-id file"),
        format!("{commit}\n")
    );
    assert!(staging_paths(root.path()).is_empty());
    assert_eq!(names(&baselines), [".objects", "ws_a"]);

    std::fs::write(workspace.join("README.md"), b"changed workspace\n").expect("change");
    let untouched = observe(&driver).await;
    assert_eq!(
        untouched
            .1
            .as_ref()
            .expect("baseline file")
            .content
            .decode()
            .expect("content"),
        b"bounded v2 workspace\n"
    );
    std::fs::remove_dir_all(workspace.join(".git/objects")).expect("object store removed");
    assert_eq!(observe(&driver).await, untouched);
}

/// A create whose root name already has a commit-id file or a store is refused
/// `workspace.git-target-exists` before any request and before either entry is written.
#[tokio::test]
async fn a_create_whose_root_name_has_a_commit_id_file_or_a_store_is_refused_before_any_request() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    let baselines = baselines(root.path());
    std::fs::write(baselines.join("ws_file"), b"another workspace's\n").expect("commit-id file");
    let store = baselines.join(".objects/ws_store");
    std::fs::create_dir_all(&store).expect("store");
    std::fs::write(store.join("marker"), b"another workspace's\n").expect("entry");

    for name in ["ws_file", "ws_store"] {
        let outcome = driver
            .create_workspace_authorized(name, name, &input(&server, commit), Some(AUTHORITY))
            .await;
        let (dispatch, error) = refused(outcome);
        assert_eq!(error.code, "workspace.git-target-exists", "{name}");
        assert_eq!(dispatch, "contained absent", "{name}");
        assert!(!root.path().join(name).exists(), "{name}");
    }
    assert!(
        server.requests.lock().expect("requests").is_empty(),
        "the refusal precedes every request"
    );
    assert_eq!(
        std::fs::read(baselines.join("ws_file")).expect("commit-id file"),
        b"another workspace's\n"
    );
    assert_eq!(names(&baselines), [".objects", "ws_file"]);
    assert_eq!(names(&baselines.join(".objects")), ["ws_store"]);
    assert_eq!(names(&store), ["marker"]);
    assert!(staging_paths(root.path()).is_empty());
}

/// A create that finds its root name taken when it comes to write the baseline is refused
/// `workspace.git-target-exists`, and removes neither entry of the workspace that took it.
#[tokio::test]
async fn a_create_whose_root_name_is_taken_while_it_fetches_removes_no_entry_of_the_other() {
    let (source, commit) = repository(0);
    let root = tempdir().expect("workspace root");
    let taken = baselines(root.path());
    let server = Server::start_observed(
        source.path(),
        Mode::V2,
        Some(Arc::new(move || {
            let store = taken.join(".objects/ws_a");
            if std::fs::create_dir_all(&store).is_ok() {
                let _ = std::fs::write(store.join("marker"), b"another workspace's\n");
                let _ = std::fs::write(taken.join("ws_a"), b"another workspace's\n");
            }
        })),
    );
    let driver = driver(root.path(), &server);
    let baselines = baselines(root.path());

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;
    let (dispatch, error) = refused(outcome);
    assert_eq!(error.code, "workspace.git-target-exists");
    assert_eq!(dispatch, "contained absent");
    assert_eq!(server.requests.lock().expect("requests").len(), 3);
    assert_eq!(
        std::fs::read(baselines.join("ws_a")).expect("the other workspace's commit-id file"),
        b"another workspace's\n"
    );
    assert_eq!(names(&baselines.join(".objects/ws_a")), ["marker"]);
    assert_eq!(names(&baselines), [".objects", "ws_a"]);
    assert_eq!(names(&baselines.join(".objects")), ["ws_a"]);
    assert!(staging_paths(root.path()).is_empty());
}

/// A driver with a Git source configured that destroys a Git workspace removes its commit-id
/// file and its store with it, and nothing of another Git workspace.
#[tokio::test]
async fn destroying_a_git_workspace_removes_its_commit_id_file_and_its_store() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    let baselines = baselines(root.path());
    for name in ["ws_a", "ws_b"] {
        let created = driver
            .create_workspace_authorized(name, name, &input(&server, commit), Some(AUTHORITY))
            .await;
        assert!(matches!(created, DispatchOutcome::Observed(_)), "{name}");
    }
    assert_eq!(names(&baselines), [".objects", "ws_a", "ws_b"]);
    assert_eq!(names(&baselines.join(".objects")), ["ws_a", "ws_b"]);
    let kept = objects_of(&baselines.join(".objects/ws_b"));

    let mut absent = false;
    for _ in 0..100 {
        absent = matches!(
            driver
                .destroy_workspace("ws_a", "ws_a")
                .await
                .expect("destroy"),
            WorkspaceDestroyProgress::Absent(_)
        );
        if absent {
            break;
        }
    }
    assert!(absent, "the destroy finished");
    assert!(!root.path().join("ws_a").exists());
    assert_eq!(names(&baselines), [".objects", "ws_b"]);
    assert_eq!(names(&baselines.join(".objects")), ["ws_b"]);
    assert_eq!(objects_of(&baselines.join(".objects/ws_b")), kept);
}

/// A failed create leaves no commit-id file and no store, also when its staging tree cannot be
/// removed.
#[tokio::test]
async fn a_failed_create_leaves_no_commit_id_file_and_no_store_also_when_its_staging_stays() {
    let (source, commit) = repository(0);
    let root = tempdir().expect("workspace root");
    let workspaces = root.path().to_owned();
    // While the create fetches, a directory of its root name appears, so the install fails,
    // and its staging tree gets an entry the create cannot remove.
    let server = Server::start_observed(
        source.path(),
        Mode::V2,
        Some(Arc::new(move || {
            let _ = std::fs::create_dir(workspaces.join("ws_a"));
            for staging in staging_paths(&workspaces) {
                let kept = staging.join("kept");
                if std::fs::create_dir(&kept).is_ok() {
                    let _ = std::fs::write(kept.join("file"), b"kept");
                    let _ = std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o555));
                }
            }
        })),
    );
    let driver = driver(root.path(), &server);

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;
    // Lets the test's own directory be removed.
    for staging in staging_paths(root.path()) {
        let _ =
            std::fs::set_permissions(staging.join("kept"), std::fs::Permissions::from_mode(0o755));
    }
    let (_, error) = refused(outcome);
    assert_eq!(
        server.requests.lock().expect("requests").len(),
        3,
        "the create fetched before it failed: {}",
        error.code
    );
    let baselines = baselines(root.path());
    assert!(
        !baselines.join("ws_a").exists(),
        "a commit-id file is left after {}",
        error.code
    );
    assert_eq!(names(&baselines.join(".objects")), Vec::<String>::new());
    assert!(
        names(&baselines).iter().all(|name| name == ".objects"),
        "{:?}",
        names(&baselines)
    );
}

/// A create refuses a root name outside the root-name set before dispatch: nothing is fetched
/// and nothing is written.
#[tokio::test]
async fn a_create_refuses_a_root_name_outside_the_root_name_set_before_dispatch() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    let before = (names(root.path()), names(&baselines(root.path())));

    for name in [".hidden", "a/b", "..", "-dash", "", ".objects"] {
        let outcome = driver
            .create_workspace_authorized("ws_a", name, &input(&server, commit), Some(AUTHORITY))
            .await;
        let (dispatch, error) = refused(outcome);
        assert_eq!(dispatch, "not dispatched", "{name:?}: {}", error.code);
        assert_eq!(error.code, "workspace.path-escape", "{name:?}");
    }
    assert!(server.requests.lock().expect("requests").is_empty());
    assert_eq!((names(root.path()), names(&baselines(root.path()))), before);
}

/// The workspace a create was served with.
pub(super) fn served(outcome: DispatchOutcome<Workspace>) -> Workspace {
    match outcome {
        DispatchOutcome::Observed(workspace) => workspace,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("the create was refused {}", error.code)
        }
    }
}

/// Design 21, § *Nesting*: a create whose baseline nests directories exactly as deep as the
/// bound is served. Its store is written, and both observations answer from it.
#[tokio::test]
async fn a_create_whose_baseline_nests_directories_as_deep_as_the_bound_is_served() {
    let (source, commit) = nested_repository(NESTING_BOUND);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);

    served(
        driver
            .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
            .await,
    );

    let baselines = baselines(root.path());
    // The commit, its tree, the file and one tree for each nested directory.
    assert_eq!(
        objects_of(&baselines.join(".objects/ws_a")).len(),
        NESTING_BOUND + 3
    );
    assert_eq!(
        std::fs::read_to_string(baselines.join("ws_a")).expect("commit-id file"),
        format!("{commit}\n")
    );
    let (changes, file) = observe(&driver).await;
    assert_eq!(changes, Vec::new());
    assert_eq!(
        file.expect("baseline file")
            .content
            .decode()
            .expect("content"),
        b"bounded git workspace\n"
    );
}

/// Design 21, § *Nesting*: a create whose baseline nests directories deeper than the bound is
/// refused `workspace.git-path-unrepresentable`, and leaves no store, no commit-id file and no
/// workspace.
#[tokio::test]
async fn a_create_whose_baseline_nests_directories_deeper_than_the_bound_is_refused() {
    let (source, commit) = nested_repository(NESTING_BOUND + 1);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;

    let (dispatch, error) = refused(outcome);
    assert_eq!(error.code, "workspace.git-path-unrepresentable");
    assert_eq!(dispatch, "contained absent");
    assert!(!root.path().join("ws_a").exists(), "a workspace is left");
    let baselines = baselines(root.path());
    assert!(!baselines.join("ws_a").exists(), "a commit-id file is left");
    assert_eq!(names(&baselines.join(".objects")), Vec::<String>::new());
    assert!(
        names(&baselines).iter().all(|name| name == ".objects"),
        "{:?}",
        names(&baselines)
    );
    assert!(staging_paths(root.path()).is_empty());
}

/// A create whose baseline names more paths than its inode limit is refused
/// `workspace.git-storage-limit` before anything is checked out, and leaves no workspace, no
/// store and no commit-id file. The baseline here names 2^41 paths with forty-two trees,
/// against the 200,000 of a create that requests no limit.
#[tokio::test]
async fn a_create_whose_baseline_names_more_paths_than_its_inode_limit_is_refused() {
    let (source, commit) = doubled_repository(40);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;

    let (dispatch, error) = refused(outcome);
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(dispatch, "contained absent");
    assert!(!root.path().join("ws_a").exists(), "a workspace is left");
    let baselines = baselines(root.path());
    assert!(!baselines.join("ws_a").exists(), "a commit-id file is left");
    assert_eq!(names(&baselines.join(".objects")), Vec::<String>::new());
    assert!(
        names(&baselines).iter().all(|name| name == ".objects"),
        "{:?}",
        names(&baselines)
    );
    assert!(staging_paths(root.path()).is_empty());
}

/// A Git workspace with a commit-id file and no store has no Git observation through the
/// driver: both are refused `workspace.git-baseline-missing`, and nothing is made for it. The
/// workspace here was created with its store, which is then removed.
#[tokio::test]
async fn a_git_workspace_without_a_store_has_no_git_observation_and_nothing_is_made_for_it() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    served(
        driver
            .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
            .await,
    );
    let baselines = baselines(root.path());
    std::fs::remove_dir_all(baselines.join(".objects/ws_a")).expect("store removed");

    let changes = driver
        .read_workspace_git_changes_v2("ws_a", "ws_a", &QUERY)
        .await
        .map_or_else(|error| error.code, |_| "answered");
    let file = driver
        .read_workspace_git_baseline_file_v2("ws_a", "ws_a", "README.md", 1024)
        .await
        .map_or_else(|error| error.code, |_| "answered");

    assert_eq!([changes, file], ["workspace.git-baseline-missing"; 2]);
    assert_eq!(names(&baselines.join(".objects")), Vec::<String>::new());
    assert_eq!(names(&baselines), [".objects", "ws_a"]);
    assert!(staging_paths(root.path()).is_empty());
}

/// The file `zeros.bin` of `length` zero bytes, alone in a commit on `provider-default`.
fn repository_of_zeros(length: usize) -> (tempfile::TempDir, git2::Oid) {
    repository_holding(|repository| {
        let file = repository.blob(&vec![0; length]).expect("file");
        let mut root = repository.treebuilder(None).expect("tree builder");
        root.insert("zeros.bin", file, 0o100_644).expect("file");
        root.write().expect("root tree")
    })
}

/// The packs and pack indexes in the object store of the staging tree at `target`.
pub(super) fn fetched_packs(target: &Path) -> Vec<String> {
    names(&target.join(".git/objects/pack"))
        .into_iter()
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "pack" || extension == "idx")
        })
        .collect()
}

/// The fetch allocates for one object no more than the byte limit of the create
/// (`gitoxide.objects.allocLimit`): an entry of the pack that states more is refused
/// `workspace.git-storage-limit` while the fetch indexes the pack, before the bytes it states
/// are allocated. The file here is 2 MiB of zeros, which the pack holds in about 2 KiB, so the
/// transfer is within every limit. Under a byte limit of 1 MiB the fetch ends without a pack
/// in the staging tree and without the branch, so nothing read the object afterwards; under
/// one of 4 MiB the same source is fetched and checked out.
#[test]
fn an_object_that_states_more_than_the_byte_limit_ends_the_fetch_before_it_is_allocated() {
    const FILE: usize = 2 * 1024 * 1024;
    let (source, commit) = repository_of_zeros(FILE);
    let server = Server::start(source.path(), Mode::V2);
    let fetch = |max_bytes: u64| {
        let target = tempdir().expect("target");
        let outcome = crate::git::materialize(
            target.path(),
            &server.locator,
            &server.binding,
            &server.source(commit),
            &zeroize::Zeroizing::new(AUTHORITY.to_owned()),
            substrate_wire::StorageLimit {
                max_bytes,
                max_inodes: 2000,
            },
            &Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        (outcome, target)
    };

    let (outcome, target) = fetch(1024 * 1024);
    let error = outcome.expect_err("an object over the byte limit");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, crate::DriverErrorClass::Exhausted);
    assert_eq!(
        fetched_packs(target.path()),
        Vec::<String>::new(),
        "the fetch finished its pack"
    );
    assert!(
        git2::Repository::open(target.path())
            .expect("staging repository")
            .refname_to_id("refs/remotes/origin/provider-default")
            .is_err(),
        "the fetch wrote the branch"
    );
    assert!(!target.path().join("zeros.bin").exists());

    let (outcome, target) = fetch(4 * 1024 * 1024);
    outcome.expect("an object within the byte limit");
    assert_eq!(fetched_packs(target.path()).len(), 2);
    assert_eq!(
        std::fs::metadata(target.path().join("zeros.bin"))
            .expect("checked-out file")
            .len(),
        u64::try_from(FILE).expect("file length")
    );
}

/// An observation takes one of the places the driver keeps for file operations, and waits
/// for no other observation. The commit-id file of `ws_b` is a FIFO here: an observation of
/// `ws_b` takes its place and then waits there for its commit id, until the case writes it.
/// Opening the FIFO for writing returns when the observation has opened it for reading, which
/// is how the case knows that the observation is in its place; no time is waited for. One
/// place is taken then. With every other place but one held, both observations of `ws_a`
/// answer while the observation of `ws_b` still waits. Once its commit id is written, that
/// one answers too. Each of the two observations is held that way once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_observation_takes_one_place_and_waits_for_no_other_observation() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    for name in ["ws_a", "ws_b"] {
        served(
            driver
                .create_workspace_authorized(name, name, &input(&server, commit), Some(AUTHORITY))
                .await,
        );
    }
    let fifo = baselines(root.path()).join("ws_b");
    std::fs::remove_file(&fifo).expect("commit-id file removed");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).expect("FIFO");
    let places = driver.blocking_slots.available_permits();

    for held_observation in ["changes", "baseline file"] {
        let observing = Arc::clone(&driver);
        let observation = tokio::spawn(async move {
            if held_observation == "changes" {
                observing
                    .read_workspace_git_changes_v2("ws_b", "ws_b", &QUERY)
                    .await
                    .map(|_| ())
            } else {
                observing
                    .read_workspace_git_baseline_file_v2("ws_b", "ws_b", "README.md", 1024)
                    .await
                    .map(|_| ())
            }
        });
        let opened = fifo.clone();
        let mut commit_id = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(opened)
        })
        .await
        .expect("the FIFO is opened")
        .expect("the FIFO, open for writing");
        assert_eq!(
            driver.blocking_slots.available_permits(),
            places - 1,
            "{held_observation}: the places one observation holds"
        );
        let others = Arc::clone(&driver.blocking_slots)
            .acquire_many_owned(u32::try_from(places - 2).expect("places"))
            .await
            .expect("every other place but one");

        let (changes, file) = observe(&driver).await;

        assert_eq!(changes, Vec::new(), "{held_observation}");
        assert!(file.is_some(), "{held_observation}: the baseline file");
        assert!(
            !observation.is_finished(),
            "{held_observation}: the observation ended without its commit id"
        );
        drop(others);
        commit_id
            .write_all(format!("{commit}\n").as_bytes())
            .expect("commit id written");
        drop(commit_id);
        let answered = observation.await.expect("observation");
        assert_eq!(
            answered.map_err(|error| error.code),
            Ok(()),
            "{held_observation}"
        );
    }
    assert_eq!(driver.blocking_slots.available_permits(), places);
}
