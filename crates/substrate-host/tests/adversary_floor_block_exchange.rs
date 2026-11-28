//! Adversary pass on ADR 0050: the ways to `EXT4_IOC_MOVE_EXT` the lane case does not take.
//!
//! `floor_refuses_block_exchange` makes the native call once, from one exec. These cases make it
//! on the same two allocated workspace files from an exec, from a raw-pipe session and from a pty
//! session (the story's outcome names "a confined run or session"; ADR 0050 lists a session as
//! not staged), and each time through every entry a process of the run has to `ioctl`:
//!
//! * the native call, and the same request with every bit above its low word set;
//! * the x32 entry, whose `ioctl` is number 514 with the ABI bit, not 16;
//! * the 32-bit entry, `int 0x80` with number 54, whose request number and structure are the
//!   native ones, so `ext4` would serve it if the program let it pass;
//! * `XFS_IOC_SWAPEXT` on the same descriptor;
//! * `ptrace` asked to suspend a child's seccomp program, and `io_uring_setup`.
//!
//! Two control calls keep a run that ran nothing, or a program that refuses every `ioctl`, from
//! passing: `TIOCSETD` on descriptor -1 answers `EACCES`, and `FS_IOC_FIEMAP` on descriptor -1
//! reaches the kernel and answers `EBADF`. After each run both files are read cold on the host.
//!
//! One more case holds ADR 0050 decision 1's own rule against a request its survey does not
//! name: `F2FS_IOC_SEC_TRIM_FILE`, read in the kernel source and not run on f2fs. It asserts the
//! floor refuses it, and the program lets it through.
//!
//! The probe is this test executable, copied into the workspace and started inside the sandbox
//! with `adversary_probe_entry` selected, as `seccomp_review_pass1` does.
//!
//! Delegated lane only. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, or without bubblewrap, every case is **absent, never reported as passed**
//! (invariant 3): it prints `ABSENT` and its reason.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest as _, Sha256};
use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    CaptureMode, ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode,
    PipeSessionStartInput, PtyWindow, SandboxProfile, SessionMode, WorkspaceAccess,
};

const NAME: &str = "adversary floor block exchange";
const PROBE_VARIABLE: &str = "SUBSTRATE_ADVERSARY_FLOOR_PROBE";
const PROBE_FILE: &str = "adversary-floor-probe";
const WORKSPACE: &str = "ws_advfloor";

/// `_IOWR('f', 15, struct move_extent)`, written out and not taken from the driver.
const EXT4_IOC_MOVE_EXT: u32 = 0xC028_660F;
/// `_IOWR('X', 109, struct xfs_swapext)`.
const XFS_IOC_SWAPEXT: u32 = 0xC0C0_586D;
/// `_IOWR('f', 11, struct fiemap)`: a request the program lets through.
const FS_IOC_FIEMAP: u32 = 0xC020_660B;
/// `asm-generic/ioctls.h`.
const TIOCSETD: u32 = 0x5423;
/// `PTRACE_O_SUSPEND_SECCOMP`, `1 << 21`.
const SUSPEND_SECCOMP: usize = 0x0020_0000;
/// `_IOW(0xf5, 20, struct f2fs_sectrim_range)`, 24 bytes (`include/uapi/linux/f2fs.h`).
///
/// Read in `fs/f2fs/file.c` at v6.18, `f2fs_sec_trim_file` and `f2fs_secure_erase`, and not run
/// on f2fs: the descriptor has to be open for writing and nothing else is asked of the caller;
/// the call drops the file's cached pages and discards or zeroes the blocks the file keeps
/// mapped, and sets neither the size nor a time of the inode. By ADR 0050 decision 1's rule
/// that is a request the floor refuses; its survey read f2fs and names `F2FS_IOC_MOVE_RANGE`
/// and `F2FS_IOC_DEFRAGMENT` only.
const F2FS_IOC_SEC_TRIM_FILE: u32 = 0x4018_F514;

/// Sixteen blocks of 4 KiB, the range the probe asks the file system to exchange.
const LENGTH: usize = 16 * 4096;

/// `struct move_extent` of `fs/ext4/ext4.h`: 40 bytes on every target, the 32-bit one included.
#[repr(C)]
struct MoveExtent {
    reserved: u32,
    donor_fd: u32,
    orig_start: u64,
    donor_start: u64,
    len: u64,
    moved_len: u64,
}

fn move_extent(donor: libc::c_int) -> MoveExtent {
    MoveExtent {
        reserved: 0,
        donor_fd: donor.cast_unsigned(),
        orig_start: 0,
        donor_start: 0,
        len: 16,
        moved_len: 0,
    }
}

/// `OK`, or the errno of a failed call.
fn answer(result: libc::c_long) -> String {
    if result >= 0 {
        "OK".to_owned()
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        format!("ERRNO {errno}")
    }
}

fn say(step: &str, answer: &str) {
    println!("ADV {step} {answer}");
}

/// The raw syscall and not `libc::ioctl`, so that all 64 bits of `request` reach the kernel.
fn raw_ioctl(descriptor: libc::c_int, request: u64, argument: *mut libc::c_void) -> String {
    // SAFETY: `argument` is null or points at storage of the request's size that outlives the
    // call.
    answer(unsafe {
        libc::syscall(
            libc::SYS_ioctl,
            libc::c_long::from(descriptor),
            request.cast_signed(),
            argument,
        )
    })
}

/// `ioctl` through the 32-bit entry: `int 0x80` with the i386 number 54 in `eax`, and the low
/// words of `ebx`, `ecx` and `edx` as its arguments. Returns what the kernel left in `eax`.
///
/// # Safety
///
/// `argument` names memory below 4 GiB that holds the request's structure, or nothing the
/// kernel would read.
#[cfg(target_arch = "x86_64")]
unsafe fn int80_ioctl(descriptor: u32, request: u32, argument: u32) -> i32 {
    let mut eax: u64 = 54;
    // SAFETY: `rbx` cannot be named as an operand, so the descriptor travels in `rsi` and is
    // exchanged into `rbx` around the trap; the kernel keeps `rsi` across it. `r8` to `r11` are
    // declared lost because older kernels return them zeroed from this entry.
    unsafe {
        core::arch::asm!(
            "xchg rbx, rsi",
            "int 0x80",
            "xchg rbx, rsi",
            inout("rax") eax,
            inout("rsi") u64::from(descriptor) => _,
            in("rcx") u64::from(request),
            in("rdx") u64::from(argument),
            lateout("r8") _,
            lateout("r9") _,
            lateout("r10") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    let bytes = eax.to_ne_bytes();
    i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// `EXT4_IOC_MOVE_EXT` on `orig` through the 32-bit entry, made in a forked child because a
/// kernel without that entry answers the trap with a signal.
#[cfg(target_arch = "x86_64")]
fn through_the_32_bit_entry(orig: libc::c_int, donor: libc::c_int) -> String {
    // SAFETY: an anonymous private mapping below 4 GiB, checked before use.
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_32BIT,
            -1,
            0,
        )
    };
    if page == libc::MAP_FAILED {
        return "SKIP no mapping below 4 GiB".to_owned();
    }
    let Ok(low) = u32::try_from(page.addr()) else {
        return "SKIP the mapping is not below 4 GiB".to_owned();
    };
    // SAFETY: the mapping is one writable page, aligned for the structure.
    unsafe { page.cast::<MoveExtent>().write(move_extent(donor)) };
    // SAFETY: the child makes one trap and exits without running a destructor.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return "SKIP fork failed".to_owned();
    }
    if child == 0 {
        // SAFETY: `low` names the page written above.
        let result = unsafe { int80_ioctl(orig.cast_unsigned(), EXT4_IOC_MOVE_EXT, low) };
        let code = i32::try_from(result.unsigned_abs()).unwrap_or(255).min(255);
        // SAFETY: the post-fork child ends here.
        unsafe { libc::_exit(code) };
    }
    let mut status = 0;
    // SAFETY: `child` is the pid fork returned and `status` is writable.
    if unsafe { libc::waitpid(child, &raw mut status, 0) } != child {
        return "SKIP the child was not reaped".to_owned();
    }
    if libc::WIFSIGNALED(status) {
        format!("SIGNAL {}", libc::WTERMSIG(status))
    } else {
        match libc::WEXITSTATUS(status) {
            0 => "OK".to_owned(),
            errno => format!("ERRNO {errno}"),
        }
    }
}

/// Asks `ptrace` to seize a child with its seccomp program suspended: the one request that takes
/// a process out from under its program. Refused before the attach for a caller that is itself
/// under a program or holds no `CAP_SYS_ADMIN` in the initial namespace.
fn suspend_a_childs_program() -> String {
    // SAFETY: the child only waits to be killed.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return "SKIP fork failed".to_owned();
    }
    if child == 0 {
        loop {
            // SAFETY: `pause` takes no argument.
            let _interrupted = unsafe { libc::pause() };
        }
    }
    // SAFETY: a seize of this process's own child with no address and one option.
    let said = answer(unsafe {
        libc::ptrace(
            libc::PTRACE_SEIZE,
            child,
            std::ptr::null_mut::<libc::c_void>(),
            std::ptr::without_provenance_mut::<libc::c_void>(SUSPEND_SECCOMP),
        )
    });
    let mut status = 0;
    // SAFETY: `child` is this process's own child.
    unsafe {
        libc::kill(child, libc::SIGKILL);
        libc::waitpid(child, &raw mut status, 0);
    }
    said
}

/// Every process the run's `/proc` lists, held to seccomp mode 2 (a filter): `FILTERED` and
/// their number, or `UNFILTERED` and each one that is not. Bubblewrap's own first process of the
/// namespace is one of them, and a process without the program could be traced into making the
/// call for the run.
fn processes_outside_the_program() -> String {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return "SKIP /proc does not list".to_owned();
    };
    let mut seen = 0_u32;
    let mut outside = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let field = |key: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|value| value.trim().to_owned())
                .unwrap_or_default()
        };
        seen += 1;
        if field("Seccomp:") != "2" {
            outside.push(format!("{name}:{}:{}", field("Name:"), field("Seccomp:")));
        }
    }
    if outside.is_empty() {
        format!("FILTERED {seen}")
    } else {
        format!("UNFILTERED {}", outside.join(","))
    }
}

/// What runs inside the sandbox. A plain test run selects it too and it returns at once.
#[test]
fn adversary_probe_entry() {
    if std::env::var_os(PROBE_VARIABLE).is_none() {
        return;
    }
    let mut discipline: libc::c_int = 0;
    say(
        "control-tiocsetd",
        &raw_ioctl(-1, u64::from(TIOCSETD), (&raw mut discipline).cast()),
    );
    say(
        "control-fiemap",
        &raw_ioctl(-1, u64::from(FS_IOC_FIEMAP), std::ptr::null_mut()),
    );
    let open = |name: &str| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("/workspace/{name}"))
    };
    let (orig, donor) = match (open("orig"), open("donor")) {
        (Ok(orig), Ok(donor)) => (orig, donor),
        (orig, donor) => {
            say(
                "open",
                &format!("FAILED {:?} {:?}", orig.err(), donor.err()),
            );
            return;
        }
    };
    say("open", "OK");
    let (orig, donor) = (orig.as_raw_fd(), donor.as_raw_fd());

    let mut request = move_extent(donor);
    say(
        "native",
        &raw_ioctl(
            orig,
            u64::from(EXT4_IOC_MOVE_EXT),
            (&raw mut request).cast(),
        ),
    );
    let mut request = move_extent(donor);
    say(
        "high-word",
        &raw_ioctl(
            orig,
            u64::from(EXT4_IOC_MOVE_EXT) | (u64::MAX << 32),
            (&raw mut request).cast(),
        ),
    );
    #[cfg(target_arch = "x86_64")]
    {
        let mut request = move_extent(donor);
        // SAFETY: the x32 `ioctl` entry with a descriptor, the request and its structure.
        let x32 = unsafe {
            libc::syscall(
                0x4000_0000 | 0x202,
                libc::c_long::from(orig),
                libc::c_long::from(EXT4_IOC_MOVE_EXT),
                &raw mut request,
            )
        };
        say("x32", &answer(x32));
        say("i386", &through_the_32_bit_entry(orig, donor));
    }
    let mut swap = [0_u8; 192];
    say(
        "swapext",
        &raw_ioctl(orig, u64::from(XFS_IOC_SWAPEXT), swap.as_mut_ptr().cast()),
    );
    say("suspend-seccomp", &suspend_a_childs_program());
    let mut parameters = [0_u8; 128];
    // SAFETY: `io_uring_setup` with one entry and zeroed parameters of the structure's size.
    let ring = unsafe {
        libc::syscall(
            libc::SYS_io_uring_setup,
            libc::c_long::from(1_i32),
            parameters.as_mut_ptr(),
        )
    };
    say("io-uring-setup", &answer(ring));
    say("every-process", &processes_outside_the_program());
    say(
        "survey-f2fs-sec-trim",
        &raw_ioctl(-1, u64::from(F2FS_IOC_SEC_TRIM_FILE), std::ptr::null_mut()),
    );
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

/// One file of `LENGTH` bytes, all `byte`, with its blocks allocated.
fn allocated(path: &Path, byte: u8) {
    std::fs::write(path, vec![byte; LENGTH]).expect("the file is written");
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .expect("the file is on disk");
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    orig: PathBuf,
    donor: PathBuf,
    pty: bool,
    file_system: String,
    _directory: tempfile::TempDir,
}

/// The driver on a delegated root with one workspace holding the probe and the two files, or the
/// reason the case is absent.
fn lane() -> Result<Lane, String> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap".to_owned());
    }
    // Under the target directory, as the lane case stages: on `ext4` the request exists.
    let directory = tempfile::Builder::new()
        .prefix("adversary-floor-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("host root");
    let file_system = format!(
        "{:?}",
        nix::sys::statfs::statfs(directory.path())
            .expect("statfs")
            .filesystem_type()
    );
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = Some(root);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        return Err("the driver published no confined exec".to_owned());
    }
    let workspace = driver.root().join(WORKSPACE);
    std::fs::create_dir_all(&workspace).expect("workspace directory");
    let probe = workspace.join(PROBE_FILE);
    std::fs::copy(std::env::current_exe().expect("test executable"), &probe)
        .expect("copy the probe into the workspace");
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755))
        .expect("probe is executable");
    let (orig, donor) = (workspace.join("orig"), workspace.join("donor"));
    allocated(&orig, b'X');
    allocated(&donor, b'V');
    let pty = machine.facts.sessions_pty == Some(true);
    Ok(Lane {
        driver,
        snapshot: machine.snapshot,
        orig,
        donor,
        pty,
        file_system,
        _directory: directory,
    })
}

fn exec_input(snapshot: &str, wait: bool) -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: [
            &format!("/workspace/{PROBE_FILE}"),
            "adversary_probe_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "--quiet",
        ]
        .iter()
        .map(|part| (*part).to_owned())
        .collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::from([(PROBE_VARIABLE.to_owned(), "1".to_owned())]),
        },
        sandbox: ConfinementRequest {
            requires: None,
            capability_snapshot: snapshot.to_owned(),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
            profile: SandboxProfile::Workspace,
            required: true,
            exec_subpaths: None,
            exec_denied_paths: Vec::new(),
            local_endpoint_key: None,
        },
        limits: ExecLimits {
            timeout_ms: 30_000,
            output_bytes: 65_536,
            processes: 16,
            memory_bytes: 268_435_456,
            cpu_millis: 10_000,
        },
        wait,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: Some(60_000),
        workdir: None,
    }
}

/// The probe's answers by step, read out of everything the run printed. A terminal ends a line
/// with a carriage return and the test harness prints its own words before the first answer.
fn answers(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = &line[line.find("ADV ")? + 4..];
            let (step, answer) = rest.split_once(' ')?;
            Some((step.to_owned(), answer.to_owned()))
        })
        .collect()
}

/// Holds one run's answers to the refusals and both files to their state before it.
fn hold(run: &str, output: &str, before: &(Cold, Cold), after: &(Cold, Cold)) {
    let refused = format!("ERRNO {}", libc::EACCES);
    let mut said = answers(output);
    println!("{NAME}: {run}: {said:?}");
    // Held by its own case below, so that a request the list lacks does not hide these answers.
    let _survey = said.remove("survey-f2fs-sec-trim");
    let processes = said.remove("every-process").unwrap_or_default();
    assert!(
        processes
            .strip_prefix("FILTERED ")
            .and_then(|count| count.parse::<u32>().ok())
            .is_some_and(|count| count >= 2),
        "{NAME}: {run}: a process of the run's pid namespace is not under a seccomp filter, or \
         fewer than two were listed (bubblewrap's first process and the probe): {processes:?}; \
         everything the run printed: {output:?}"
    );
    let suspended = said.remove("suspend-seccomp").unwrap_or_default();
    assert!(
        suspended.starts_with("ERRNO "),
        "{NAME}: {run}: ptrace took a child out from under its seccomp program: {suspended:?}; \
         everything the run printed: {output:?}"
    );
    let mut expected = BTreeMap::from([
        ("control-tiocsetd".to_owned(), refused.clone()),
        (
            "control-fiemap".to_owned(),
            format!("ERRNO {}", libc::EBADF),
        ),
        ("open".to_owned(), "OK".to_owned()),
        ("native".to_owned(), refused.clone()),
        ("high-word".to_owned(), refused.clone()),
        ("swapext".to_owned(), refused.clone()),
        ("io-uring-setup".to_owned(), refused.clone()),
    ]);
    if cfg!(target_arch = "x86_64") {
        expected.insert("x32".to_owned(), refused.clone());
        expected.insert("i386".to_owned(), refused.clone());
        if said
            .get("i386")
            .is_some_and(|answer| answer.starts_with("SIGNAL "))
        {
            println!(
                "{NAME}: {run}: this kernel has no 32-bit entry ({}); absent, not passed: i386",
                said["i386"]
            );
            said.insert("i386".to_owned(), refused);
        }
    }
    assert_eq!(
        said, expected,
        "{NAME}: {run}: an entry to EXT4_IOC_MOVE_EXT or XFS_IOC_SWAPEXT was not refused with \
         EACCES, or a control call did not answer as the program must; everything the run \
         printed: {output:?}"
    );
    assert_eq!(before.0, after.0, "{NAME}: {run}: the first file changed");
    assert_eq!(before.1, after.1, "{NAME}: {run}: the second file changed");
}

/// Starts the probe as a session of `mode` and returns everything it printed until end of file.
async fn session_output(lane: &Lane, id: &str, mode: SessionMode) -> String {
    let input = PipeSessionStartInput {
        capture: CaptureMode::Recorded,
        exec: exec_input(&lane.snapshot, false),
        input_limit_bytes: 65_536,
        frame_limit_bytes: 4_096,
        queued_frames: 8,
        mode,
        window: (mode == SessionMode::Pty).then_some(PtyWindow {
            columns: 200,
            rows: 50,
        }),
    };
    match lane.driver.start_pipe_session(id, WORKSPACE, &input).await {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "{NAME}: the delegated lane must start session {id}: {} {}",
            error.code, error.message
        ),
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut transcript = Vec::new();
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "{NAME}: session {id} did not end within 30 s: {:?}",
            String::from_utf8_lossy(&transcript)
        );
        match lane
            .driver
            .read_pipe_session(id, Duration::from_millis(500))
            .await
        {
            Ok(None) => break,
            Ok(Some(frame)) => transcript.extend_from_slice(&frame.bytes),
            Err(_timeout) => {}
        }
    }
    String::from_utf8_lossy(&transcript).into_owned()
}

/// Starts the probe as one exec that is waited for and returns everything it printed.
async fn exec_output(lane: &Lane, id: &str) -> String {
    let observed = match lane
        .driver
        .start_exec(id, WORKSPACE, &exec_input(&lane.snapshot, true))
        .await
    {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "{NAME}: the delegated lane must dispatch exec {id}: {} {}",
            error.code, error.message
        ),
    };
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&observed.stdout),
        String::from_utf8_lossy(&observed.stderr)
    )
}

/// ADR 0050 decision 1 states the rule for the list: a request is refused when a process without
/// a capability can use it to change the content of a file with no stat fact of the changed file
/// moving. `F2FS_IOC_SEC_TRIM_FILE` is such a request (see the constant), on a file system the
/// survey says it read, and the program lets it through: on descriptor -1 the kernel answers
/// `EBADF`, where a refused request answers `EACCES`.
#[tokio::test(flavor = "multi_thread")]
async fn the_rule_of_adr_0050_refuses_the_f2fs_request_that_erases_a_files_blocks() {
    let started = std::time::Instant::now();
    let lane = match lane() {
        Ok(lane) => lane,
        Err(reason) => {
            println!("{NAME}: f2fs survey: ABSENT ({reason})");
            return;
        }
    };
    let output = exec_output(&lane, "ex_advfloor_survey").await;
    let said = answers(&output);
    let refused = format!("ERRNO {}", libc::EACCES);
    assert_eq!(
        said.get("control-tiocsetd"),
        Some(&refused),
        "{NAME}: f2fs survey: the probe did not run under the program: {output:?}"
    );
    println!(
        "{NAME}: f2fs survey: ioctl(-1, F2FS_IOC_SEC_TRIM_FILE) answered {:?}, {} ms",
        said.get("survey-f2fs-sec-trim"),
        started.elapsed().as_millis()
    );
    assert_eq!(
        said.get("survey-f2fs-sec-trim"),
        Some(&refused),
        "{NAME}: f2fs survey: F2FS_IOC_SEC_TRIM_FILE (0x4018F514) passes the floor: it needs a \
         descriptor open for writing and no capability, erases the blocks a file keeps mapped \
         and moves no stat fact (fs/f2fs/file.c at v6.18, read and not run on f2fs), which is \
         ADR 0050 decision 1's rule for a refused request"
    );
    println!("{NAME}: f2fs survey: executed");
}

#[tokio::test(flavor = "multi_thread")]
async fn no_entry_of_an_exec_reaches_the_block_exchange() {
    let started = std::time::Instant::now();
    let lane = match lane() {
        Ok(lane) => lane,
        Err(reason) => {
            println!("{NAME}: exec: ABSENT ({reason})");
            return;
        }
    };
    let before = (cold(&lane.orig), cold(&lane.donor));
    let output = exec_output(&lane, "ex_advfloor_exec").await;
    let after = (cold(&lane.orig), cold(&lane.donor));
    hold("exec", &output, &before, &after);
    println!(
        "{NAME}: exec: executed on {}, {} ms",
        lane.file_system,
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_entry_of_a_pipe_session_reaches_the_block_exchange() {
    let started = std::time::Instant::now();
    let lane = match lane() {
        Ok(lane) => lane,
        Err(reason) => {
            println!("{NAME}: pipe session: ABSENT ({reason})");
            return;
        }
    };
    let before = (cold(&lane.orig), cold(&lane.donor));
    let output = session_output(&lane, "ex_advfloor_pipes", SessionMode::Pipes).await;
    let after = (cold(&lane.orig), cold(&lane.donor));
    hold("pipe session", &output, &before, &after);
    println!(
        "{NAME}: pipe session: executed on {}, {} ms",
        lane.file_system,
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_entry_of_a_pty_session_reaches_the_block_exchange() {
    let started = std::time::Instant::now();
    let lane = match lane() {
        Ok(lane) => lane,
        Err(reason) => {
            println!("{NAME}: pty session: ABSENT ({reason})");
            return;
        }
    };
    if !lane.pty {
        println!("{NAME}: pty session: ABSENT (the driver published no sessions.pty)");
        return;
    }
    let before = (cold(&lane.orig), cold(&lane.donor));
    let output = session_output(&lane, "ex_advfloor_pty", SessionMode::Pty).await;
    let after = (cold(&lane.orig), cold(&lane.donor));
    hold("pty session", &output, &before, &after);
    println!(
        "{NAME}: pty session: executed on {}, {} ms",
        lane.file_system,
        started.elapsed().as_millis()
    );
}
