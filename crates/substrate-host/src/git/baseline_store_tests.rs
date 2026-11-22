//! Cases of the baseline store (design 21, § *The baseline store*) through the production
//! driver and the real TLS/upload-pack peer.

use std::collections::{BTreeMap, BTreeSet};
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
use crate::git::tests::store_failure_cases::{NESTING_BOUND, copy_tree, nested_repository};
use crate::git::tests::watch::{Event, Kind, Watch};
use crate::{
    DispatchOutcome, Driver as _, DriverError, HostConfig, HostDriver, WorkspaceDestroyProgress,
};

fn input(server: &Server, commit: git2::Oid) -> WorkspaceCreateInput {
    WorkspaceCreateInput {
        source: WorkspaceSource::Git(GitSourceEnvelope {
            git: server.source(commit),
        }),
        labels: BTreeMap::default(),
        storage: None,
        lease_ttl_ms: None,
    }
}

fn driver(root: &Path, server: &Server) -> Arc<HostDriver> {
    let mut config = HostConfig::minimum(root);
    config.git_sources.push(server.binding.clone());
    HostDriver::open(config).expect("host driver")
}

fn baselines(root: &Path) -> PathBuf {
    root.join(".substrate-git-baselines")
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

/// The staging trees of Git creates in the directory of all workspaces.
fn staging_paths(root: &Path) -> Vec<PathBuf> {
    names(root)
        .into_iter()
        .filter(|name| name.starts_with(".substrate-git-") && name != ".substrate-git-baselines")
        .map(|name| root.join(name))
        .collect()
}

/// How a create that was not served ended, and its refusal.
fn refused(outcome: DispatchOutcome<Workspace>) -> (&'static str, DriverError) {
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
fn served(outcome: DispatchOutcome<Workspace>) -> Workspace {
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

/// A driver with the Git workspace `ws_a`, created from the server, and beside it one Git
/// workspace for each name in `predating`, each a copy of `ws_a` with its commit-id file and no
/// store: a workspace materialized before the store existed.
async fn driver_with_predating(
    root: &Path,
    server: &Server,
    commit: git2::Oid,
    predating: &[String],
) -> Arc<HostDriver> {
    let driver = driver(root, server);
    served(
        driver
            .create_workspace_authorized("ws_a", "ws_a", &input(server, commit), Some(AUTHORITY))
            .await,
    );
    let baselines = baselines(root);
    for name in predating {
        copy_tree(&root.join("ws_a"), &root.join(name));
        std::fs::copy(baselines.join("ws_a"), baselines.join(name)).expect("commit-id file");
    }
    driver
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

/// A Git workspace with a commit-id file and no store has no Git observation: both are refused
/// `workspace.git-baseline-missing`, and nothing is made for it. An observation takes one of
/// the places the driver keeps for file operations and waits for no other observation: with
/// every place but one held, sixteen observations of eight such workspaces all end in that
/// refusal, and both observations of `ws_a`, which has its store, answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_observation_takes_one_place_and_waits_for_no_other_observation() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let predating: Vec<String> = (0..8).map(|n| format!("ws_w{n:02}")).collect();
    let driver = driver_with_predating(root.path(), &server, commit, &predating).await;
    let baselines = baselines(root.path());
    let entries = names(&baselines);
    assert_eq!(entries.len(), 10, "{entries:?}");
    let places = driver.blocking_slots.available_permits();
    let held = Arc::clone(&driver.blocking_slots)
        .acquire_many_owned(u32::try_from(places - 1).expect("places"))
        .await
        .expect("every place but one");

    let mut observations = Vec::new();
    for name in &predating {
        let (changes, name_of_changes) = (Arc::clone(&driver), name.clone());
        observations.push(tokio::spawn(async move {
            changes
                .read_workspace_git_changes_v2(&name_of_changes, &name_of_changes, &QUERY)
                .await
                .map_or_else(|error| error.code, |_| "answered")
        }));
        let (file, name_of_file) = (Arc::clone(&driver), name.clone());
        observations.push(tokio::spawn(async move {
            file.read_workspace_git_baseline_file_v2(
                &name_of_file,
                &name_of_file,
                "README.md",
                1024,
            )
            .await
            .map_or_else(|error| error.code, |_| "answered")
        }));
    }
    for observation in observations {
        assert_eq!(
            observation.await.expect("observation"),
            "workspace.git-baseline-missing"
        );
    }
    let (changes, file) = observe(&driver).await;
    drop(held);

    assert_eq!(changes, Vec::new());
    assert!(
        file.is_some(),
        "the baseline file of the workspace with a store"
    );
    assert_eq!(names(&baselines.join(".objects")), ["ws_a"]);
    assert_eq!(names(&baselines), entries);
    assert!(staging_paths(root.path()).is_empty());
}
