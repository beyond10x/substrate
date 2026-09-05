//! Hard-quota lifecycle tests use the production driver and the real TLS/upload-pack peer.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use substrate_wire::{
    EmptySource, GitSourceEnvelope, StorageLimit, StorageUsage, Workspace, WorkspaceCreateInput,
    WorkspaceSource,
};
use tempfile::{TempDir, tempdir};

use super::{AUTHORITY, LIMIT, Mode, Server, repository};
use crate::{DispatchOutcome, Driver as _, HostConfig, HostDriver, WorkspaceDestroyProgress};

fn input(
    server: &Server,
    commit: git2::Oid,
    storage: Option<StorageLimit>,
) -> WorkspaceCreateInput {
    WorkspaceCreateInput {
        source: WorkspaceSource::Git(GitSourceEnvelope {
            git: server.source(commit),
        }),
        labels: BTreeMap::default(),
        storage,
        lease_ttl_ms: None,
    }
}

fn driver(root: &Path, server: &Server, range: Option<(u32, u32)>) -> Arc<HostDriver> {
    let mut config = HostConfig::minimum(root);
    config.git_sources.push(server.binding.clone());
    config.project_quota_ids = range;
    HostDriver::open(config).expect("host driver")
}

fn observed(outcome: DispatchOutcome<Workspace>) -> Workspace {
    match outcome {
        DispatchOutcome::Observed(workspace) => workspace,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!("workspace creation failed: {error}"),
    }
}

#[tokio::test]
async fn absent_quota_is_refused_before_any_git_network_request() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server, None);
    let result = driver
        .create_workspace_authorized(
            "ws_absent",
            "ws_absent",
            &input(&server, commit, Some(LIMIT)),
            Some(AUTHORITY),
        )
        .await;
    assert!(
        server.requests.lock().expect("requests").is_empty(),
        "an unserved quota must prevent even Git discovery"
    );
    let DispatchOutcome::NotDispatched(error) = result else {
        panic!("missing hard quotas must refuse before dispatch");
    };
    assert_eq!(error.code, "workspace.storage-quota-unserved");
    assert!(!root.path().join("ws_absent").exists());
    assert!(staging_paths(root.path()).is_empty());
}

#[tokio::test]
async fn omitted_storage_keeps_the_existing_git_workspace_contract() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server, None);
    let workspace = observed(
        driver
            .create_workspace_authorized(
                "ws_legacy",
                "ws_legacy",
                &input(&server, commit, None),
                Some(AUTHORITY),
            )
            .await,
    );
    assert!(workspace.storage.is_none());
    assert!(
        driver
            .observe_workspace("ws_legacy", "ws_legacy", &workspace)
            .await
            .expect("observe unquotaed workspace")
            .storage
            .is_none()
    );
    assert_eq!(
        git2::Repository::open(root.path().join("ws_legacy"))
            .expect("Git workspace")
            .head()
            .expect("HEAD")
            .target(),
        Some(commit)
    );
}

struct Fixture {
    root: TempDir,
    range: (u32, u32),
}

impl Fixture {
    fn open() -> Self {
        let root = std::env::var_os("SUBSTRATE_TEST_QUOTA_ROOT")
            .expect("SUBSTRATE_TEST_QUOTA_ROOT is required for the real quota lane");
        let range = std::env::var("SUBSTRATE_TEST_PROJECT_QUOTA_IDS")
            .expect("SUBSTRATE_TEST_PROJECT_QUOTA_IDS is required for the real quota lane");
        let (start, end) = range.split_once('-').expect("START-END quota range");
        let range = (
            start.parse::<u32>().expect("range start"),
            end.parse::<u32>().expect("range end"),
        );
        assert!(range.0 > 0 && range.1 >= range.0 && range.1 - range.0 >= 127);
        let root = tempfile::Builder::new()
            .prefix("git-quota-")
            .tempdir_in(root)
            .expect("private root on quota filesystem");
        assert!(
            crate::quota::ProjectQuotas::probe(root.path(), Some(range)),
            "fixture must prove actual byte, inode and inheritance enforcement; no skipped proof"
        );
        eprintln!(
            "real quota fixture root={} ids={}-{}",
            root.path().display(),
            range.0,
            range.1
        );
        Self { root, range }
    }
}

#[repr(C)]
#[derive(Default)]
struct Fsxattr {
    flags: u32,
    extent_size: u32,
    extents: u32,
    project: u32,
    cow_extent_size: u32,
    padding: [u8; 8],
}

fn project_id(path: &Path) -> u32 {
    let file = File::open(path).expect("inode for project identity");
    let mut attrs = Fsxattr::default();
    let request = nix::request_code_read!(b'X', 31, std::mem::size_of::<Fsxattr>());
    // SAFETY: the ioctl writes one live, correctly sized Linux fsxattr value.
    assert_eq!(
        unsafe { libc::ioctl(file.as_raw_fd(), request, &raw mut attrs) },
        0,
        "project identity: {}",
        std::io::Error::last_os_error()
    );
    attrs.project
}

fn kernel_quota(root: &Path, project: u32) -> libc::dqblk {
    let file = File::open(root).expect("quota filesystem fd");
    // SAFETY: zero is valid for every integer member of the Linux quota structure.
    let mut quota: libc::dqblk = unsafe { std::mem::zeroed() };
    // SAFETY: the fd is live and the syscall writes a correctly sized libc quota structure.
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_quotactl_fd,
                file.as_raw_fd(),
                libc::QCMD(libc::Q_GETQUOTA, 2),
                project,
                &raw mut quota,
            )
        },
        0,
        "kernel quota query: {}",
        std::io::Error::last_os_error()
    );
    quota
}

fn assert_kernel_usage(root: &Path, project: u32, usage: StorageUsage) {
    let quota = kernel_quota(root, project);
    assert_eq!(usage.used_bytes, quota.dqb_curspace);
    assert_eq!(usage.used_inodes, quota.dqb_curinodes);
    assert_eq!(usage.limit.max_bytes / 1024, quota.dqb_bhardlimit);
    assert_eq!(usage.limit.max_inodes, quota.dqb_ihardlimit);
}

fn staging_paths(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .expect("workspace entries")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            let name = path.file_name().expect("entry name").to_string_lossy();
            name.starts_with(".substrate-git-") && name != ".substrate-git-baselines"
        })
        .collect()
}

fn tree_projects(root: &Path) -> Vec<u32> {
    let mut paths = vec![root.to_owned()];
    let mut projects = Vec::new();
    while let Some(path) = paths.pop() {
        projects.push(project_id(&path));
        if path.is_dir() {
            paths.extend(
                std::fs::read_dir(&path)
                    .expect("tree")
                    .map(|entry| entry.expect("entry").path()),
            );
        }
    }
    projects
}

async fn destroy(driver: &HostDriver, name: &str) {
    for _ in 0..100 {
        if matches!(
            driver
                .destroy_workspace(name, name)
                .await
                .expect("destroy quota workspace"),
            WorkspaceDestroyProgress::Absent(_)
        ) {
            return;
        }
    }
    panic!("bounded fixture cleanup did not finish");
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_precedes_git_writes_and_survives_install_restart_destroy() {
    let fixture = Fixture::open();
    let (source, commit) = repository(0);
    let witnessed = Arc::new(Mutex::new(None));
    let witness = Arc::clone(&witnessed);
    let root = fixture.root.path().to_path_buf();
    let server = Server::start_observed(
        source.path(),
        Mode::V2,
        Some(Arc::new(move || {
            let mut witness = witness.lock().expect("first-request witness");
            if witness.is_none() {
                let staging = staging_paths(&root);
                assert_eq!(staging.len(), 1);
                *witness = Some(tree_projects(&staging[0]));
            }
        })),
    );
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    let workspace = observed(
        driver
            .create_workspace_authorized(
                "ws_quota",
                "ws_quota",
                &input(&server, commit, Some(LIMIT)),
                Some(AUTHORITY),
            )
            .await,
    );
    let target = fixture.root.path().join("ws_quota");
    let project = project_id(&target);
    assert!(
        (fixture.range.0..=fixture.range.1).contains(&project),
        "installed Git tree must have a delegated identity"
    );
    let projects = witnessed
        .lock()
        .expect("witness")
        .clone()
        .expect("request observed");
    assert!(projects.len() > 1, "Git had initialized before discovery");
    assert!(
        projects.iter().all(|id| *id == project),
        "every first Git inode must already inherit the installed quota"
    );
    assert!(staging_paths(fixture.root.path()).is_empty());
    assert_kernel_usage(
        fixture.root.path(),
        project,
        workspace.storage.expect("kernel usage"),
    );
    std::fs::write(target.join("later"), vec![42; 8192]).expect("later bounded mutation");
    let updated = driver
        .observe_workspace("ws_quota", "ws_quota", &workspace)
        .await
        .expect("observe installed quota");
    assert_kernel_usage(
        fixture.root.path(),
        project,
        updated.storage.expect("updated usage"),
    );
    let config = driver.config.clone();
    drop(driver);
    let recovered = HostDriver::open(config).expect("restart quota recovery");
    let updated = recovered
        .observe_workspace("ws_quota", "ws_quota", &workspace)
        .await
        .expect("observe after restart");
    assert_eq!(project_id(&target), project);
    assert_kernel_usage(
        fixture.root.path(),
        project,
        updated.storage.expect("recovered usage"),
    );
    destroy(&recovered, "ws_quota").await;
    assert_released(fixture.root.path(), project);
    assert!(!target.exists());
    let replacement = WorkspaceCreateInput {
        source: WorkspaceSource::Empty(EmptySource::Empty),
        labels: BTreeMap::default(),
        storage: Some(LIMIT),
        lease_ttl_ms: None,
    };
    observed(
        recovered
            .create_workspace("ws_reused", "ws_reused", &replacement)
            .await,
    );
    assert_eq!(
        project_id(&fixture.root.path().join("ws_reused")),
        project,
        "destroy releases the allocator path as well as disk usage"
    );
    destroy(&recovered, "ws_reused").await;
}

fn assert_released(root: &Path, project: u32) {
    assert_ne!(
        project, 0,
        "the failed operation must have attached a real quota"
    );
    assert!(staging_paths(root).is_empty(), "private staging remains");
    let quota = kernel_quota(root, project);
    assert_eq!(
        (
            quota.dqb_curspace,
            quota.dqb_curinodes,
            quota.dqb_bhardlimit,
            quota.dqb_ihardlimit
        ),
        (0, 0, 0, 0),
        "only absence plus zero kernel usage permits release"
    );
}

fn identity_observer(root: PathBuf, identity: Arc<AtomicU32>) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        let staging = staging_paths(&root);
        assert_eq!(staging.len(), 1);
        identity.store(project_id(&staging[0]), Ordering::Relaxed);
    })
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_failed_fetch_removes_staging_and_releases_its_identity() {
    let fixture = Fixture::open();
    let (source, commit) = repository(0);
    let project = Arc::new(AtomicU32::new(0));
    let server = Server::start_observed(
        source.path(),
        Mode::TruncatedPack,
        Some(identity_observer(
            fixture.root.path().to_owned(),
            Arc::clone(&project),
        )),
    );
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    let result = driver
        .create_workspace_authorized(
            "ws_failed",
            "ws_failed",
            &input(&server, commit, Some(LIMIT)),
            Some(AUTHORITY),
        )
        .await;
    let DispatchOutcome::ContainedAbsent(error) = result else {
        panic!("failed fetch must prove the workspace absent")
    };
    assert!(!error.to_string().contains(AUTHORITY));
    assert!(!fixture.root.path().join("ws_failed").exists());
    assert_released(fixture.root.path(), project.load(Ordering::Relaxed));
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_install_conflict_preserves_the_other_tree_and_releases_staging() {
    let fixture = Fixture::open();
    let (source, commit) = repository(0);
    let project = Arc::new(AtomicU32::new(0));
    let identity = identity_observer(fixture.root.path().to_owned(), Arc::clone(&project));
    let target = fixture.root.path().join("ws_conflict");
    let competing = target.clone();
    let server = Server::start_observed(
        source.path(),
        Mode::V2,
        Some(Arc::new(move || {
            identity();
            if !competing.exists() {
                std::fs::create_dir(&competing).expect("competing workspace");
                std::fs::write(competing.join("owner"), b"other operation")
                    .expect("other owner's data");
            }
        })),
    );
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    let result = driver
        .create_workspace_authorized(
            "ws_conflict",
            "ws_conflict",
            &input(&server, commit, Some(LIMIT)),
            Some(AUTHORITY),
        )
        .await;
    let DispatchOutcome::OutcomeUnknown(error) = result else {
        panic!("the competing target remains observable")
    };
    assert_eq!(error.code, "workspace.git-target-exists");
    assert_eq!(
        std::fs::read(target.join("owner")).expect("preserved other tree"),
        b"other operation"
    );
    assert_released(fixture.root.path(), project.load(Ordering::Relaxed));
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_cancelled_fetch_releases_staging_after_the_worker_stops() {
    let fixture = Fixture::open();
    let (source, commit) = repository(0);
    let project = Arc::new(AtomicU32::new(0));
    let identity = identity_observer(fixture.root.path().to_owned(), Arc::clone(&project));
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let reached = Arc::clone(&entered);
    let resumed = Arc::clone(&release);
    let server = Server::start_observed(
        source.path(),
        Mode::V2,
        Some(Arc::new(move || {
            identity();
            reached.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !resumed.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        })),
    );
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    let task_driver = Arc::clone(&driver);
    let input = input(&server, commit, Some(LIMIT));
    let worker = tokio::spawn(async move {
        task_driver
            .create_workspace_authorized("ws_cancelled", "ws_cancelled", &input, Some(AUTHORITY))
            .await
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entered.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "materialization did not reach the bounded peer"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    worker.abort();
    let Err(join_error) = worker.await else {
        panic!("async dispatch must be cancelled");
    };
    assert!(join_error.is_cancelled());
    release.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !staging_paths(fixture.root.path()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "cancelled worker retained staging"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!fixture.root.path().join("ws_cancelled").exists());
    assert_released(fixture.root.path(), project.load(Ordering::Relaxed));
}

fn exhaust_bytes(path: &Path, limit: StorageLimit) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path.join("byte-ceiling"))
        .expect("bounded writer");
    let mut refused = false;
    let block = vec![17; 65_536];
    for _ in 0..=limit.max_bytes / 65536 {
        if let Err(error) = file.write_all(&block) {
            assert_eq!(
                error.raw_os_error(),
                Some(libc::EDQUOT),
                "hard quota and backing filesystem exhaustion remain distinct"
            );
            refused = true;
            break;
        }
    }
    assert!(refused, "later writes crossed the workspace byte ceiling");
}

fn exhaust_inodes(path: &Path, limit: StorageLimit) {
    let mut refused = false;
    for number in 0..=limit.max_inodes {
        if let Err(error) = File::create(path.join(format!("inode-{number}"))) {
            assert_eq!(error.raw_os_error(), Some(libc::EDQUOT));
            refused = true;
            break;
        }
    }
    assert!(
        refused,
        "empty-file writes crossed the workspace inode ceiling"
    );
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_byte_and_inode_limits_enforce_later_workspace_mutations() {
    let fixture = Fixture::open();
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    for (name, limit, exhaust) in [
        (
            "ws_bytes",
            StorageLimit {
                max_bytes: 1024 * 1024,
                ..LIMIT
            },
            exhaust_bytes as fn(&Path, StorageLimit),
        ),
        (
            "ws_inodes",
            StorageLimit {
                max_inodes: 64,
                ..LIMIT
            },
            exhaust_inodes as fn(&Path, StorageLimit),
        ),
    ] {
        let workspace = observed(
            driver
                .create_workspace_authorized(
                    name,
                    name,
                    &input(&server, commit, Some(limit)),
                    Some(AUTHORITY),
                )
                .await,
        );
        let path = fixture.root.path().join(name);
        let project = project_id(&path);
        exhaust(&path, limit);
        let updated = driver
            .observe_workspace(name, name, &workspace)
            .await
            .expect("quota usage after refusal");
        let usage = updated.storage.expect("kernel usage");
        assert!(usage.used_bytes <= limit.max_bytes);
        assert!(usage.used_inodes <= limit.max_inodes);
        assert_kernel_usage(fixture.root.path(), project, usage);
        destroy(&driver, name).await;
        assert_released(fixture.root.path(), project);
    }
}

fn repository_with_large_checkout() -> (TempDir, git2::Oid) {
    let (source, parent) = repository(0);
    let repository = git2::Repository::open(source.path()).expect("large checkout source");
    let blob = repository
        .blob(&vec![b'Q'; 2 * 1024 * 1024])
        .expect("compressible blob");
    let mut builder = repository.treebuilder(None).expect("tree builder");
    builder
        .insert("large.txt", blob, 0o100_644)
        .expect("large file");
    let tree = repository
        .find_tree(builder.write().expect("tree write"))
        .expect("large checkout tree");
    let signature =
        git2::Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
    let parent = repository.find_commit(parent).expect("parent commit");
    let commit = repository
        .commit(
            Some("refs/heads/provider-default"),
            &signature,
            &signature,
            "checkout expands beyond a small quota",
            &tree,
            &[&parent],
        )
        .expect("large checkout commit");
    (source, commit)
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_checkout_exhaustion_cannot_release_another_live_session() {
    let fixture = Fixture::open();
    let (source, commit) = repository_with_large_checkout();
    let project = Arc::new(AtomicU32::new(0));
    let server = Server::start_observed(
        source.path(),
        Mode::V2,
        Some(identity_observer(
            fixture.root.path().to_owned(),
            Arc::clone(&project),
        )),
    );
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    let live = observed(
        driver
            .create_workspace_authorized(
                "ws_live",
                "ws_live",
                &input(&server, commit, Some(LIMIT)),
                Some(AUTHORITY),
            )
            .await,
    );
    let live_path = fixture.root.path().join("ws_live");
    let live_project = project_id(&live_path);
    assert_eq!(
        std::fs::metadata(live_path.join("large.txt"))
            .unwrap()
            .len(),
        2 * 1024 * 1024
    );
    let small_limit = StorageLimit {
        max_bytes: 1024 * 1024,
        ..LIMIT
    };
    let failed = driver
        .create_workspace_authorized(
            "ws_exhausted",
            "ws_exhausted",
            &input(&server, commit, Some(small_limit)),
            Some(AUTHORITY),
        )
        .await;
    let DispatchOutcome::ContainedAbsent(error) = failed else {
        panic!("quota must stop checkout and leave no failed workspace");
    };
    assert_eq!(
        error.code, "workspace.git-checkout-failed",
        "post-checkout scanning is too late"
    );
    let failed_project = project.load(Ordering::Relaxed);
    assert_ne!(failed_project, live_project);
    assert_released(fixture.root.path(), failed_project);
    assert!(!fixture.root.path().join("ws_exhausted").exists());
    let live = driver
        .observe_workspace("ws_live", "ws_live", &live)
        .await
        .expect("other session remains quota-bound");
    assert_kernel_usage(fixture.root.path(), live_project, live.storage.unwrap());
    assert_eq!(project_id(&live_path), live_project);
    destroy(&driver, "ws_live").await;
    assert_released(fixture.root.path(), live_project);
}

#[tokio::test]
#[ignore = "requires the explicitly delegated ext4 project-quota fixture"]
async fn real_quota_concurrent_installs_keep_independent_identities_and_release() {
    let fixture = Fixture::open();
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let driver = driver(fixture.root.path(), &server, Some(fixture.range));
    let input = input(&server, commit, Some(LIMIT));
    let (first, second) = tokio::join!(
        driver.create_workspace_authorized("ws_first", "ws_first", &input, Some(AUTHORITY)),
        driver.create_workspace_authorized("ws_second", "ws_second", &input, Some(AUTHORITY)),
    );
    let first = observed(first);
    let second = observed(second);
    let first_path = fixture.root.path().join("ws_first");
    let second_path = fixture.root.path().join("ws_second");
    let first_project = project_id(&first_path);
    let second_project = project_id(&second_path);
    assert_ne!(first_project, second_project);
    for (path, project, usage) in [
        (&first_path, first_project, first.storage.unwrap()),
        (&second_path, second_project, second.storage.unwrap()),
    ] {
        assert!(tree_projects(path).iter().all(|id| *id == project));
        assert_kernel_usage(fixture.root.path(), project, usage);
    }
    destroy(&driver, "ws_first").await;
    assert_released(fixture.root.path(), first_project);
    let second = driver
        .observe_workspace("ws_second", "ws_second", &second)
        .await
        .expect("remaining session survives selective destruction");
    assert_kernel_usage(fixture.root.path(), second_project, second.storage.unwrap());
    observed(
        driver
            .create_workspace_authorized(
                "ws_replacement",
                "ws_replacement",
                &input,
                Some(AUTHORITY),
            )
            .await,
    );
    assert_eq!(
        project_id(&fixture.root.path().join("ws_replacement")),
        first_project
    );
    assert_eq!(project_id(&second_path), second_project);
    destroy(&driver, "ws_second").await;
    destroy(&driver, "ws_replacement").await;
    assert_released(fixture.root.path(), second_project);
    assert_released(fixture.root.path(), first_project);
}
