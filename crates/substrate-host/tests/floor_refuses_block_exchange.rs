//! The confinement floor refuses the `ioctl` request that exchanges the blocks of two files
//! (ADR 0050, `story:floor-refuses-the-ioctl-that-exchanges-file-blocks`).
//!
//! One confined run on the real `linux` adapter, with a writable workspace on `ext4`, starts a
//! static program that calls `EXT4_IOC_MOVE_EXT` on two files of that workspace. The case holds
//! when the call answers `EACCES` and both files keep their content and their five stat facts
//! (device, inode, size, modification time, change time), read on the host after the data pages
//! were dropped: until then a reader gets the old content from the page cache, which is what
//! makes the exchange invisible.
//!
//! Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is inside,
//! without bubblewrap, without a `cc` that links a static program, or where the test's own
//! directory is not on `ext4`, the case is **absent, never reported as passed** (invariant 3):
//! it prints `ABSENT` and its reason. `bash scripts/delegated-lane.sh` runs it and requires its
//! `executed` line, or the one reason it accepts.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementProfile, Fact, ProcessBounds,
    RunState, linux::LinuxRequest,
};

const NAME: &str = "floor block exchange";

/// Sixteen blocks of 4 KiB: what the program asks the file system to exchange.
const LENGTH: usize = 16 * 4096;

/// The program a run starts. `EXT4_IOC_MOVE_EXT` is `_IOWR('f', 15, struct move_extent)`, which
/// is `0xC028660F` on both supported targets; the structure is spelled out because the request
/// is not in the C library's headers. It prints what the call answered and exits 0 either way,
/// so the case reads the answer and not an exit status.
const MOVER: &str = r#"
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

struct move_extent {
    unsigned int reserved;
    unsigned int donor_fd;
    unsigned long long orig_start;
    unsigned long long donor_start;
    unsigned long long len;
    unsigned long long moved_len;
};

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    int orig = open(argv[1], O_RDWR);
    int donor = open(argv[2], O_RDWR);
    if (orig < 0 || donor < 0) {
        printf("open errno=%d\n", errno);
        return 3;
    }
    struct move_extent request;
    memset(&request, 0, sizeof request);
    request.donor_fd = (unsigned int)donor;
    request.len = 16;
    int rc = ioctl(orig, _IOWR('f', 15, struct move_extent), &request);
    int error = rc == 0 ? 0 : errno;
    printf("request=0x%lX rc=%d errno=%d moved_len=%llu\n",
           (unsigned long)_IOWR('f', 15, struct move_extent), rc, error, request.moved_len);
    return 0;
}
"#;

/// The delegated cgroup v2 subtree this process is inside, or the reason the case is absent.
fn delegated() -> Result<PathBuf, String> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap".to_owned());
    }
    Ok(root)
}

/// `MOVER` built with the host's `cc` as a statically linked program, or the reason it is not.
/// Static, because the base view of a run is not what the case is about and a loader would be
/// one more thing the run has to find.
fn mover(directory: &Path) -> Result<PathBuf, String> {
    let source = directory.join("mover.c");
    let program = directory.join("mover");
    std::fs::write(&source, MOVER).expect("the source");
    let built = std::process::Command::new("cc")
        .arg("-static")
        .arg("-o")
        .arg(&program)
        .arg(&source)
        .output()
        .map_err(|error| format!("no C compiler named cc: {error}"))?;
    if !built.status.success() {
        return Err(format!(
            "cc built no static program: {}",
            String::from_utf8_lossy(&built.stderr).trim()
        ));
    }
    assert_eq!(
        substrate_exec_gate::elf_linkage(&std::fs::read(&program).expect("the program")),
        substrate_exec_gate::Linkage::Static,
        "{NAME}: cc -static built a program with a PT_INTERP"
    );
    Ok(program)
}

/// What the host reads of one file once its data pages are dropped.
#[derive(Debug, PartialEq, Eq)]
struct Cold {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    digest: String,
    distinct_bytes: BTreeSet<u8>,
}

impl Cold {
    /// The five stat facts, without the content.
    fn facts(&self) -> (u64, u64, u64, (i64, i64), (i64, i64)) {
        (
            self.device,
            self.inode,
            self.size,
            self.modified,
            self.changed,
        )
    }
}

/// Writes the file's data back, drops its cached pages, and reads it and its stat facts.
fn cold(path: &Path) -> Cold {
    let file = std::fs::File::open(path).expect("the file opens");
    file.sync_data().expect("the data is written back");
    nix::fcntl::posix_fadvise(
        &file,
        0,
        0,
        nix::fcntl::PosixFadviseAdvice::POSIX_FADV_DONTNEED,
    )
    .expect("the cached pages are dropped");
    let bytes = std::fs::read(path).expect("the file reads");
    let facts = std::fs::metadata(path).expect("the file has stat facts");
    Cold {
        device: facts.dev(),
        inode: facts.ino(),
        size: facts.size(),
        modified: (facts.mtime(), facts.mtime_nsec()),
        changed: (facts.ctime(), facts.ctime_nsec()),
        digest: hex::encode(Sha256::digest(&bytes)),
        distinct_bytes: bytes.iter().copied().collect(),
    }
}

/// One file of `LENGTH` bytes, all `byte`, with its blocks allocated: the file system answers
/// `ENODATA` for a range it has not yet allocated, which would pass for a refusal.
fn allocated(path: &Path, byte: u8) {
    std::fs::write(path, vec![byte; LENGTH]).expect("the file is written");
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .expect("the file is on disk");
}

/// The number after `key` in the program's line.
fn answered(line: &str, key: &str) -> i64 {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("{NAME}: the program's line has no {key}: {line:?}"))
}

/// Starts the program as one confined run with `workspace` writable, and returns its line.
async fn run_mover(adapter: &Arc<dyn ConfinementAdapter>, workspace: &Path) -> String {
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.to_path_buf()),
            facts: vec![Fact::WholeTreeKill, Fact::IpcSignalsContained],
            process_bounds: Some(ProcessBounds {
                timeout_ms: 20_000,
                processes: 16,
                memory_bytes: 128 << 20,
                cpu_millis: 10_000,
            }),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(!admitted.declared_insecure());
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/workspace/mover".into(),
                    "/workspace/orig".into(),
                    "/workspace/donor".into(),
                ],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    assert_eq!(
        ended.exit.as_ref().and_then(|exit| exit.code),
        Some(0),
        "{NAME}: the program did not reach the call: {:?} {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_cannot_exchange_the_blocks_of_two_workspace_files() {
    let started = std::time::Instant::now();
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    // Under the target directory, not the default temporary directory: that one is commonly
    // `tmpfs`, where the request is not implemented and the case would show nothing.
    let host = tempfile::Builder::new()
        .prefix("floor-block-exchange-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("host root");
    let file_system = nix::sys::statfs::statfs(host.path()).expect("statfs");
    if file_system.filesystem_type() != nix::sys::statfs::EXT4_SUPER_MAGIC {
        // The one absence `scripts/delegated-lane.sh` takes, by these words.
        println!(
            "{NAME}: ABSENT (the staging file system is not ext4: {:?})",
            file_system.filesystem_type()
        );
        return;
    }
    let program = match mover(host.path()) {
        Ok(program) => program,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let workspace = host.path().join("workspaces").join("ws_exchange");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::copy(&program, workspace.join("mover")).expect("the program is in the workspace");
    std::fs::set_permissions(
        workspace.join("mover"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("the program is executable");
    let (orig, donor) = (workspace.join("orig"), workspace.join("donor"));
    allocated(&orig, b'X');
    allocated(&donor, b'V');
    let before = (cold(&orig), cold(&donor));

    let config = HostConfig::minimum(host.path().join("workspaces"));
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    let line = run_mover(&adapter, &workspace).await;
    let after = (cold(&orig), cold(&donor));

    println!("{NAME}: the run's program said: {line}");
    println!("{NAME}: orig  before {:?}", before.0);
    println!("{NAME}: orig  after  {:?}", after.0);
    println!("{NAME}: donor before {:?}", before.1);
    println!("{NAME}: donor after  {:?}", after.1);
    println!(
        "{NAME}: content changed: {}; stat facts changed: {}; {} ms",
        before.0.digest != after.0.digest || before.1.digest != after.1.digest,
        before.0.facts() != after.0.facts() || before.1.facts() != after.1.facts(),
        started.elapsed().as_millis()
    );

    assert_eq!(
        (answered(&line, "rc="), answered(&line, "errno=")),
        (-1, i64::from(libc::EACCES)),
        "{NAME}: the floor did not refuse EXT4_IOC_MOVE_EXT with EACCES: {line}"
    );
    assert_eq!(before.0, after.0, "{NAME}: the first file changed");
    assert_eq!(before.1, after.1, "{NAME}: the second file changed");
    println!("{NAME}: executed, EXT4_IOC_MOVE_EXT answered EACCES and both files are unchanged");
}
