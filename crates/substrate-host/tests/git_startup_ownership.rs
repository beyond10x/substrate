//! Git inactivity regressions for hosts with no configured Git sources.
//!
//! Every borrowed parent and external sentinel belongs to this test's `TempDir`. No test uses a
//! repository checkout as a host root. Assertions cover Git inactivity; capsule/aperture startup
//! outside the protected sentinel trees is permitted.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{ErrorKind, Read as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::{
    DispatchOutcome, Driver as _, HostConfig, HostDriver, WorkspaceDestroyProgress,
};
use substrate_wire::{EmptySource, GitChangesQuery, WorkspaceCreateInput, WorkspaceSource};
use tempfile::TempDir;

const BASELINES: &str = ".substrate-git-baselines";

struct Fixture {
    directory: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("test-owned fixture");
        let fixture = Self { directory };
        fs::create_dir(fixture.parent()).expect("borrowed test parent");
        fs::create_dir(fixture.external()).expect("external test target");
        populate(&fixture.external());
        fixture
    }

    fn parent(&self) -> PathBuf {
        self.directory.path().join("borrowed")
    }

    fn external(&self) -> PathBuf {
        self.directory.path().join("external")
    }

    fn baselines(&self) -> PathBuf {
        self.parent().join(BASELINES)
    }

    fn config(&self) -> HostConfig {
        let mut config = HostConfig::minimum(self.parent());
        // This portable case needs no confinement backend or delegated exec resources.
        config.bubblewrap = self.directory.path().join("no-sandbox-binary");
        config
    }

    fn open_empty(&self) -> Arc<HostDriver> {
        let config = self.config();
        assert!(config.git_sources.is_empty());
        let driver = HostDriver::open(config).expect("empty-Git host must open");
        assert_eq!(driver.machine().facts.workspace_git, None);
        driver
    }
}

fn populate(directory: &Path) {
    fs::set_permissions(directory, fs::Permissions::from_mode(0o751)).expect("sentinel mode");
    fs::create_dir(directory.join("nested")).expect("sentinel directory");
    fs::write(directory.join("nested/bytes"), b"private sentinel\0\xff\n").expect("sentinel bytes");
    fs::set_permissions(
        directory.join("nested/bytes"),
        fs::Permissions::from_mode(0o640),
    )
    .expect("sentinel file mode");
    symlink("nested/bytes", directory.join("link")).expect("sentinel symlink");
}

#[derive(Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    mode: u32,
    uid: u32,
    gid: u32,
    device: u64,
    inode: u64,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn snapshot(root: &Path) -> Option<BTreeMap<PathBuf, Entry>> {
    match fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => panic!("sentinel metadata: {error}"),
    }
    let mut result = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let path = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(&relative)
        };
        let metadata = fs::symlink_metadata(&path).expect("sentinel entry metadata");
        let kind = if metadata.file_type().is_symlink() {
            EntryKind::Symlink(fs::read_link(&path).expect("sentinel link target"))
        } else if metadata.is_dir() {
            for entry in fs::read_dir(&path).expect("sentinel directory entries") {
                pending.push(relative.join(entry.expect("sentinel entry").file_name()));
            }
            EntryKind::Directory
        } else {
            assert!(metadata.is_file(), "unexpected test-owned sentinel type");
            EntryKind::File(fs::read(&path).expect("sentinel content"))
        };
        result.insert(
            relative,
            Entry {
                kind,
                mode: metadata.mode(),
                uid: metadata.uid(),
                gid: metadata.gid(),
                device: metadata.dev(),
                inode: metadata.ino(),
                links: metadata.nlink(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            },
        );
    }
    Some(result)
}

/// Observes reads as well as mutation of one protected Git-state directory.
/// The test's own snapshots happen outside the observed interval; atime is not an oracle.
struct AccessWatch(File);

impl AccessWatch {
    fn new(path: &Path) -> Self {
        // SAFETY: inotify_init1 takes only the documented flag bitset and returns an owned fd.
        let descriptor = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        assert!(
            descriptor >= 0,
            "inotify: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: the successful syscall returned a new descriptor, transferred exactly once.
        let file = unsafe { File::from_raw_fd(descriptor) };
        let path = CString::new(path.as_os_str().as_bytes()).expect("fixture path has no NUL");
        let mask = libc::IN_OPEN
            | libc::IN_ACCESS
            | libc::IN_MODIFY
            | libc::IN_ATTRIB
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO;
        // SAFETY: file owns a live inotify fd and path is a live, NUL-terminated string.
        let watch = unsafe { libc::inotify_add_watch(file.as_raw_fd(), path.as_ptr(), mask) };
        assert!(watch >= 0, "watch: {}", std::io::Error::last_os_error());
        Self(file)
    }

    fn drain(&mut self) -> usize {
        let mut observed = 0;
        let mut buffer = [0_u8; 4096];
        loop {
            match self.0.read(&mut buffer) {
                Ok(0) => panic!("inotify closed before observation"),
                Ok(count) => observed += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return observed,
                Err(error) => panic!("inotify observation: {error}"),
            }
        }
    }
}

#[tokio::test]
async fn empty_git_startup_does_not_create_baseline_state() {
    let fixture = Fixture::new();
    assert!(snapshot(&fixture.baselines()).is_none());
    let driver = fixture.open_empty();
    driver.shutdown().await.expect("host shutdown");
    assert!(snapshot(&fixture.baselines()).is_none());
}

#[tokio::test]
async fn empty_git_startup_preserves_foreign_prefix_entries() {
    let fixture = Fixture::new();
    let file = fixture.parent().join(".substrate-git-user-file");
    let directory = fixture.parent().join(".substrate-git-user-directory");
    let link = fixture.parent().join(".substrate-git-probe-user-link");
    fs::write(&file, b"unrelated file").expect("foreign file");
    fs::create_dir(&directory).expect("foreign directory");
    populate(&directory);
    symlink(fixture.external(), &link).expect("foreign prefix link");
    let paths = [file, directory, link, fixture.external()];
    let before: Vec<_> = paths.iter().map(|path| snapshot(path)).collect();
    let driver = fixture.open_empty();
    driver.shutdown().await.expect("host shutdown");
    let after: Vec<_> = paths.iter().map(|path| snapshot(path)).collect();
    assert_eq!(
        after, before,
        "a filename prefix grants no retirement authority"
    );
}

#[tokio::test]
async fn empty_git_startup_does_not_access_existing_baseline_directory() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.baselines()).expect("foreign baseline directory");
    populate(&fixture.baselines());
    let before = snapshot(&fixture.baselines());
    let mut watch = AccessWatch::new(&fixture.baselines());
    drop(fs::read_dir(fixture.baselines()).expect("calibrate directory-open observation"));
    assert!(watch.drain() > 0, "the watch must detect a directory open");
    let driver = fixture.open_empty();
    driver.shutdown().await.expect("host shutdown");
    let accessed = watch.drain();
    let after = snapshot(&fixture.baselines());
    assert_eq!(after, before, "foreign baseline tree changed");
    assert_eq!(accessed, 0, "empty Git must neither scan nor mutate state");
}

#[tokio::test]
async fn empty_git_startup_ignores_a_foreign_baseline_file() {
    let fixture = Fixture::new();
    fs::write(fixture.baselines(), b"this is not a state directory")
        .expect("foreign baseline file");
    let before = snapshot(&fixture.baselines());
    let driver = fixture.open_empty();
    driver.shutdown().await.expect("host shutdown");
    assert_eq!(snapshot(&fixture.baselines()), before);
}

#[tokio::test]
async fn empty_git_startup_ignores_a_dangling_baseline_symlink() {
    let fixture = Fixture::new();
    let absent = fixture.directory.path().join("absent-target");
    symlink(&absent, fixture.baselines()).expect("dangling baseline link");
    let before = snapshot(&fixture.baselines());
    let driver = fixture.open_empty();
    driver.shutdown().await.expect("host shutdown");
    assert_eq!(snapshot(&fixture.baselines()), before);
    assert!(
        snapshot(&absent).is_none(),
        "the dangling target must not be created"
    );
}

#[tokio::test]
async fn empty_git_startup_preserves_baseline_alias_and_external_target() {
    let fixture = Fixture::new();
    symlink(fixture.external(), fixture.baselines()).expect("baseline alias");
    let link_before = snapshot(&fixture.baselines());
    let target_before = snapshot(&fixture.external());
    let driver = fixture.open_empty();
    driver.shutdown().await.expect("host shutdown");
    assert_eq!(snapshot(&fixture.baselines()), link_before);
    assert_eq!(snapshot(&fixture.external()), target_before);
}

#[tokio::test]
async fn empty_git_observations_do_not_access_foreign_baselines() {
    let fixture = Fixture::new();
    let driver = fixture.open_empty();
    // Populate after open to isolate observation from the independently tested startup defect.
    fs::create_dir_all(fixture.baselines()).expect("foreign metadata fixture");
    fs::write(
        fixture.baselines().join("borrowed_workspace"),
        b"foreign baseline",
    )
    .expect("unowned baseline");
    fs::create_dir(fixture.parent().join("borrowed_workspace")).expect("borrowed workspace");
    let before = snapshot(&fixture.baselines());
    let mut watch = AccessWatch::new(&fixture.baselines());
    let baseline = driver
        .read_workspace_git_baseline_file_v2("borrowed_workspace", "borrowed_workspace", "file", 64)
        .await;
    let changes = driver
        .read_workspace_git_changes_v2(
            "borrowed_workspace",
            "borrowed_workspace",
            &GitChangesQuery {
                max_files: 4,
                max_file_bytes: 64,
                max_total_bytes: 256,
            },
        )
        .await;
    let accessed = watch.drain();
    driver.shutdown().await.expect("host shutdown");
    assert_eq!(snapshot(&fixture.baselines()), before);
    assert_eq!(
        accessed, 0,
        "non-Git observations must not consult foreign metadata"
    );
    assert_eq!(
        baseline.expect_err("non-Git baseline refusal").code,
        "workspace.git-workspace-required"
    );
    assert_eq!(
        changes.expect_err("non-Git changes refusal").code,
        "workspace.git-workspace-required"
    );
}

#[tokio::test]
async fn empty_git_destroy_does_not_remove_a_foreign_baseline() {
    let fixture = Fixture::new();
    let driver = fixture.open_empty();
    assert_eq!(
        driver.machine().facts.workspace_openat2_beneath,
        Some(true),
        "this case requires the declared guarded workspace mechanism"
    );
    let input = WorkspaceCreateInput {
        source: WorkspaceSource::Empty(EmptySource::Empty),
        labels: BTreeMap::new(),
        storage: None,
        lease_ttl_ms: None,
    };
    let created = driver
        .create_workspace("ws_empty", "ws_empty", &input)
        .await;
    match created {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("empty workspace creation must succeed: {error}");
        }
    }
    fs::create_dir_all(fixture.baselines()).expect("foreign baseline fixture");
    fs::write(
        fixture.baselines().join("ws_empty"),
        b"another owner's metadata",
    )
    .expect("foreign baseline");
    let before = snapshot(&fixture.baselines());
    let mut watch = AccessWatch::new(&fixture.baselines());
    let progress = driver
        .destroy_workspace("ws_empty", "ws_empty")
        .await
        .expect("empty workspace destroy");
    let accessed = watch.drain();
    driver.shutdown().await.expect("host shutdown");
    assert!(matches!(progress, WorkspaceDestroyProgress::Absent(_)));
    assert!(snapshot(&fixture.parent().join("ws_empty")).is_none());
    assert_eq!(
        snapshot(&fixture.baselines()),
        before,
        "workspace absence grants no ownership of same-name metadata"
    );
    assert_eq!(
        accessed, 0,
        "empty workspace destruction must not access Git state"
    );
}
