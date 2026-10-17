//! Portable cases for the host-process driver, plus its delegated cases.
//!
//! The delegated cases need a cgroup v2 subtree the test process is itself inside, which
//! `scripts/delegated-lane.sh` provides through `SUBSTRATE_VECTORS_CGROUP_ROOT`. Without it each
//! of them returns before asserting anything: absent, never reported as passed (invariant 3).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};

use super::log::{LogStream, LogStreamSelection, ProcessLog};
use super::ports::{
    ListeningSocket, SocketProtocol, parse_proc_net, socket_inode, sockets_for_inodes,
};
use super::{
    ARGV_INVALID, CWD_INVALID, Confinement, Deadline, DeadlineCause, ENV_INVALID,
    EXPIRES_AT_INVALID, EndedBy, HOST_UNSERVED, HostProcessSpec, HostProcessSupervisor,
    HostProfile, MAX_PROCESS_ARGV_BYTES, MAX_PROCESS_ARGV_ITEMS, MAX_PROCESS_ENV_BYTES,
    MAX_PROCESS_ENV_ITEMS, MAX_PROCESS_TIMEOUT_MS, NOT_FOUND, ProcessExit, StopCause, StopStep,
    TIMEOUT_INVALID, open_cwd,
};
use crate::DriverErrorClass;

fn spec(argv: &[&str], cwd: &Path) -> HostProcessSpec {
    HostProcessSpec {
        argv: argv.iter().map(|item| (*item).to_owned()).collect(),
        cwd: cwd.to_path_buf(),
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        deadline: None,
    }
}

fn refusal_code(result: Result<(), crate::DriverError>) -> &'static str {
    result.expect_err("the input must be refused").code
}

// ---------------------------------------------------------------------------------------------
// S1/S2: argv, env and cwd validation.
// ---------------------------------------------------------------------------------------------

#[test]
fn argv_must_be_non_empty_bounded_and_free_of_nul() {
    let cwd = Path::new("/");
    assert_eq!(refusal_code(spec(&[], cwd).validate()), ARGV_INVALID);
    assert_eq!(refusal_code(spec(&[""], cwd).validate()), ARGV_INVALID);
    assert_eq!(
        refusal_code(spec(&["/bin/echo", "a\0b"], cwd).validate()),
        ARGV_INVALID
    );
    let many = vec!["x"; MAX_PROCESS_ARGV_ITEMS + 1];
    assert_eq!(refusal_code(spec(&many, cwd).validate()), ARGV_INVALID);
    let long = "y".repeat(MAX_PROCESS_ARGV_BYTES + 1);
    assert_eq!(
        refusal_code(spec(&["/bin/echo", &long], cwd).validate()),
        ARGV_INVALID
    );
    let at_bound = vec!["x"; MAX_PROCESS_ARGV_ITEMS];
    spec(&at_bound, cwd)
        .validate()
        .expect("argv at its item bound is admitted");
    spec(&["/bin/echo", "hello world", "$HOME", "a;b"], cwd)
        .validate()
        .expect("argv is passed verbatim, never through a shell");
}

#[test]
fn env_is_the_complete_bounded_environment_with_valid_names() {
    let cwd = Path::new("/");
    let with_env = |pairs: &[(&str, &str)]| HostProcessSpec {
        env: pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        ..spec(&["/bin/true"], cwd)
    };
    with_env(&[])
        .validate()
        .expect("an empty environment is a complete environment");
    with_env(&[("PATH", "/bin"), ("npm_config_cache", "/x"), ("_A1", "")])
        .validate()
        .expect("portable names, lower case included, are admitted");
    for name in ["", "1ABC", "A=B", "A B", "A\0B", "Ä"] {
        assert_eq!(
            refusal_code(with_env(&[(name, "v")]).validate()),
            ENV_INVALID,
            "name {name:?}"
        );
    }
    assert_eq!(
        refusal_code(with_env(&[("A", "x\0y")]).validate()),
        ENV_INVALID
    );
    let names = (0..=MAX_PROCESS_ENV_ITEMS)
        .map(|index| format!("V{index}"))
        .collect::<Vec<_>>();
    let pairs = names
        .iter()
        .map(|name| (name.as_str(), "v"))
        .collect::<Vec<_>>();
    assert_eq!(refusal_code(with_env(&pairs).validate()), ENV_INVALID);
    let big = "z".repeat(MAX_PROCESS_ENV_BYTES);
    assert_eq!(
        refusal_code(with_env(&[("BIG", &big)]).validate()),
        ENV_INVALID
    );
}

#[test]
fn cwd_must_be_absolute_and_lexically_canonical() {
    for cwd in ["relative", "/a/../b", "/a/./b", "/a//b", "/a/b/", ""] {
        assert_eq!(
            refusal_code(spec(&["/bin/true"], Path::new(cwd)).validate()),
            CWD_INVALID,
            "cwd {cwd:?}"
        );
    }
    spec(&["/bin/true"], Path::new("/"))
        .validate()
        .expect("root");
}

#[test]
fn cwd_opens_only_an_existing_directory_without_following_symlinks() {
    let directory = tempfile::tempdir().expect("tempdir");
    let real = directory
        .path()
        .canonicalize()
        .expect("canonical")
        .join("real");
    std::fs::create_dir(&real).expect("real directory");
    open_cwd(&real).expect("an existing canonical directory opens");

    let link = real.with_file_name("link");
    symlink(&real, &link).expect("symlink");
    assert_eq!(open_cwd(&link).expect_err("a symlink").code, CWD_INVALID);
    assert_eq!(
        open_cwd(&link.join("nested"))
            .expect_err("through a symlink")
            .code,
        CWD_INVALID
    );

    let file = real.with_file_name("file");
    std::fs::write(&file, b"x").expect("file");
    assert_eq!(open_cwd(&file).expect_err("a file").code, CWD_INVALID);
    assert_eq!(
        open_cwd(&real.with_file_name("absent"))
            .expect_err("absent")
            .code,
        CWD_INVALID
    );
}

// ---------------------------------------------------------------------------------------------
// S2/S3: a missing delegated cgroup is a named refusal, never a fallback.
// ---------------------------------------------------------------------------------------------

#[test]
fn no_delegated_cgroup_root_refuses_host_processes_by_name() {
    let state = tempfile::tempdir().expect("state");
    let error = HostProcessSupervisor::open(None, state.path()).expect_err("no cgroup root");
    assert_eq!(error.code, HOST_UNSERVED);
    assert_eq!(error.class, DriverErrorClass::Unserved);
}

#[test]
fn a_directory_that_is_not_a_cgroup_refuses_host_processes_by_name() {
    let state = tempfile::tempdir().expect("state");
    let fake = tempfile::tempdir().expect("fake root");
    std::fs::write(fake.path().join("cgroup.procs"), b"").expect("lookalike file");
    let error =
        HostProcessSupervisor::open(Some(fake.path()), state.path()).expect_err("not a cgroup");
    assert_eq!(error.code, HOST_UNSERVED);
    assert_eq!(error.class, DriverErrorClass::Unserved);
}

// ---------------------------------------------------------------------------------------------
// S11: deadlines.
// ---------------------------------------------------------------------------------------------

#[test]
fn timeout_must_be_positive_and_within_its_maximum() {
    let now = Utc::now();
    for timeout in [0, MAX_PROCESS_TIMEOUT_MS + 1] {
        assert_eq!(
            Deadline::effective(Some(timeout), None, now)
                .expect_err("out of bounds")
                .code,
            TIMEOUT_INVALID
        );
    }
    let deadline = Deadline::effective(Some(1_500), None, now)
        .expect("valid")
        .expect("a deadline");
    assert_eq!(deadline.cause, DeadlineCause::Timeout);
    assert_eq!(deadline.at, now + TimeDelta::milliseconds(1_500));
    assert!(
        Deadline::effective(None, None, now)
            .expect("neither")
            .is_none()
    );
}

#[test]
fn expires_at_must_be_a_utc_instant_far_enough_ahead_and_within_the_maximum() {
    let now = Utc::now();
    let format =
        |at: chrono::DateTime<Utc>| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let refused = [
        "not a time".to_owned(),
        // An offset other than UTC.
        (now + TimeDelta::hours(1))
            .with_timezone(&chrono::FixedOffset::east_opt(3_600).expect("offset"))
            .to_rfc3339(),
        format(now - TimeDelta::seconds(5)),
        // Sooner than MIN_LEASE_TTL_MS.
        format(now + TimeDelta::milliseconds(200)),
        format(
            now + TimeDelta::milliseconds(
                i64::try_from(MAX_PROCESS_TIMEOUT_MS).expect("fits") + 60_000,
            ),
        ),
    ];
    for value in &refused {
        assert_eq!(
            Deadline::effective(None, Some(value), now)
                .expect_err(value)
                .code,
            EXPIRES_AT_INVALID,
            "expires_at {value:?}"
        );
    }
    let at = now + TimeDelta::minutes(5);
    let deadline = Deadline::effective(None, Some(&format(at)), now)
        .expect("valid")
        .expect("a deadline");
    assert_eq!(deadline.cause, DeadlineCause::ExpiresAt);
    assert_eq!(deadline.at.timestamp_millis(), at.timestamp_millis());
}

#[test]
fn with_both_deadlines_the_earlier_one_wins_and_names_itself() {
    let now = Utc::now();
    let later = (now + TimeDelta::minutes(10)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let sooner = (now + TimeDelta::minutes(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let timeout_first = Deadline::effective(Some(120_000), Some(&later), now)
        .expect("valid")
        .expect("deadline");
    assert_eq!(timeout_first.cause, DeadlineCause::Timeout);
    let expiry_first = Deadline::effective(Some(600_000), Some(&sooner), now)
        .expect("valid")
        .expect("deadline");
    assert_eq!(expiry_first.cause, DeadlineCause::ExpiresAt);
}

// ---------------------------------------------------------------------------------------------
// S7: the bounded log.
// ---------------------------------------------------------------------------------------------

fn log_with_bound(directory: &Path, bound: u64) -> ProcessLog {
    ProcessLog::create_bounded(&directory.join("p.log"), bound, 64).expect("log")
}

#[test]
fn log_lines_are_tagged_by_stream_and_read_back_in_order() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log_with_bound(directory.path(), 4_096);
    log.append_line(LogStream::Stdout, b"one", None)
        .expect("append");
    log.append_line(LogStream::Stderr, b"two", None)
        .expect("append");
    log.append_line(LogStream::Stdout, b"three", None)
        .expect("append");

    let both = log.tail(LogStreamSelection::Both, 1_024).expect("tail");
    let lines = both
        .lines
        .iter()
        .map(|line| (line.stream, line.bytes.as_slice()))
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            (LogStream::Stdout, b"one".as_slice()),
            (LogStream::Stderr, b"two".as_slice()),
            (LogStream::Stdout, b"three".as_slice()),
        ]
    );
    assert_eq!(both.discarded_bytes, 0);
    assert_eq!(both.confinement, Confinement::None);
    assert_eq!(both.profile, HostProfile::Host);

    let errors = log.tail(LogStreamSelection::Stderr, 1_024).expect("tail");
    assert_eq!(errors.lines.len(), 1);
    assert_eq!(errors.lines[0].bytes, b"two");
    let output = log.tail(LogStreamSelection::Stdout, 1_024).expect("tail");
    assert_eq!(output.lines.len(), 2);
}

#[test]
fn at_the_byte_bound_the_oldest_bytes_are_discarded_and_counted() {
    let directory = tempfile::tempdir().expect("dir");
    let bound = 256;
    let log = log_with_bound(directory.path(), bound);
    let mut written = 0_u64;
    for index in 0..100 {
        let line = format!("line-{index:03}");
        log.append_line(LogStream::Stdout, line.as_bytes(), None)
            .expect("append");
        // Each record is the stream tag, a space, the line and a newline.
        written += 2 + line.len() as u64 + 1;
    }
    let tail = log.tail(LogStreamSelection::Both, 4_096).expect("tail");
    assert!(
        tail.retained_bytes <= bound,
        "retained {}",
        tail.retained_bytes
    );
    assert_eq!(tail.discarded_bytes + tail.retained_bytes, written);
    assert!(tail.discarded_bytes > 0);
    let last = tail.lines.last().expect("a newest line");
    assert_eq!(last.bytes, b"line-099");
    let first = tail.lines.first().expect("an oldest retained line");
    assert_ne!(first.bytes, b"line-000", "the oldest line is gone");
    // Retained lines are whole and consecutive: nothing in the middle was dropped.
    let numbers = tail
        .lines
        .iter()
        .map(|line| {
            std::str::from_utf8(&line.bytes).expect("utf-8")["line-".len()..]
                .parse::<u32>()
                .expect("number")
        })
        .collect::<Vec<_>>();
    assert!(numbers.windows(2).all(|pair| pair[1] == pair[0] + 1));
    assert_eq!(numbers.last(), Some(&99));
}

#[test]
fn the_discarded_count_survives_reopening_the_log() {
    let directory = tempfile::tempdir().expect("dir");
    let path = directory.path().join("p.log");
    {
        let log = ProcessLog::create_bounded(&path, 128, 64).expect("log");
        for index in 0..50 {
            log.append_line(LogStream::Stderr, format!("e{index}").as_bytes(), None)
                .expect("append");
        }
    }
    let reopened = ProcessLog::open_existing(&path).expect("reopen");
    let tail = reopened
        .tail(LogStreamSelection::Both, 4_096)
        .expect("tail");
    assert!(tail.discarded_bytes > 0);
    assert_eq!(tail.lines.last().expect("line").bytes, b"e49");
    assert_eq!(tail.lines.last().expect("line").stream, LogStream::Stderr);
}

#[test]
fn a_tail_is_bounded_by_bytes_and_keeps_the_newest_lines() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log_with_bound(directory.path(), 4_096);
    for line in [b"aaaa".as_slice(), b"bbbb", b"cccc"] {
        log.append_line(LogStream::Stdout, line, None)
            .expect("append");
    }
    let tail = log.tail(LogStreamSelection::Both, 8).expect("tail");
    let lines = tail
        .lines
        .iter()
        .map(|line| line.bytes.as_slice())
        .collect::<Vec<_>>();
    assert_eq!(lines, [b"bbbb".as_slice(), b"cccc"]);
    // A newest line longer than the budget is cut from its front, and says so.
    let cut = log.tail(LogStreamSelection::Both, 2).expect("tail");
    assert_eq!(cut.lines.len(), 1);
    assert_eq!(cut.lines[0].bytes, b"cc");
    assert!(cut.lines[0].truncated);
    assert_eq!(cut.lines[0].full_len, 4);
}

#[test]
fn a_line_longer_than_the_line_bound_is_stored_cut_with_its_full_length() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log_with_bound(directory.path(), 4_096);
    let overlong = vec![b'x'; 64];
    log.append_line(LogStream::Stdout, &overlong, Some(1_000))
        .expect("append");
    let tail = log.tail(LogStreamSelection::Both, 4_096).expect("tail");
    assert_eq!(tail.lines.len(), 1);
    assert_eq!(tail.lines[0].bytes, overlong);
    assert!(tail.lines[0].truncated);
    assert_eq!(tail.lines[0].full_len, 1_000);
}

#[test]
fn the_log_file_is_owner_private_and_never_follows_a_planted_symlink() {
    let directory = tempfile::tempdir().expect("dir");
    let path = directory.path().join("p.log");
    let _log = ProcessLog::create_bounded(&path, 1_024, 64).expect("log");
    let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    let target = directory.path().join("target");
    std::fs::write(&target, b"keep").expect("target");
    let planted = directory.path().join("planted.log");
    symlink(&target, &planted).expect("symlink");
    assert!(ProcessLog::create_bounded(&planted, 1_024, 64).is_err());
    assert_eq!(std::fs::read(&target).expect("target"), b"keep");
}

// ---------------------------------------------------------------------------------------------
// S6: listening sockets from /proc/net.
// ---------------------------------------------------------------------------------------------

const PROC_NET_TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 41001 1 0000000000000000 100 0 0 10 0
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 41002 1 0000000000000000 100 0 0 10 0
   2: 0100007F:1F91 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 41003 1 0000000000000000 20 4 30 10 -1
";

const PROC_NET_TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 52001 1 0000000000000000 100 0 0 10 0
";

const PROC_NET_UDP: &str = "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  10: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 61001 2 0000000000000000 0
  11: 0100007F:A000 0100007F:0035 01 00000000:00000000 00:00000000 00000000  1000        0 61002 2 0000000000000000 0
";

#[cfg(target_endian = "little")]
#[test]
fn proc_net_tables_yield_only_listening_sockets() {
    let tcp = parse_proc_net(PROC_NET_TCP, SocketProtocol::Tcp);
    assert_eq!(
        tcp,
        [
            (41_001, IpAddr::V4(Ipv4Addr::LOCALHOST), 8_080),
            (41_002, IpAddr::V4(Ipv4Addr::UNSPECIFIED), 22),
        ]
    );
    let tcp6 = parse_proc_net(PROC_NET_TCP6, SocketProtocol::Tcp);
    assert_eq!(tcp6, [(52_001, IpAddr::V6(Ipv6Addr::LOCALHOST), 3_000)]);
    // A bound, unconnected UDP socket is the listening one; a connected one is not.
    let udp = parse_proc_net(PROC_NET_UDP, SocketProtocol::Udp);
    assert_eq!(udp, [(61_001, IpAddr::V4(Ipv4Addr::UNSPECIFIED), 5_353)]);
}

#[test]
fn malformed_proc_net_rows_are_skipped_not_guessed() {
    let garbage = "header\n 0: zz:1F90 00000000:0000 0A x x x x x 41001\n 1: 0100007F 0A\n";
    assert!(parse_proc_net(garbage, SocketProtocol::Tcp).is_empty());
}

#[test]
fn only_socket_descriptor_links_name_an_inode() {
    assert_eq!(socket_inode("socket:[41001]"), Some(41_001));
    assert_eq!(socket_inode("pipe:[41001]"), None);
    assert_eq!(socket_inode("/dev/null"), None);
    assert_eq!(socket_inode("socket:[]"), None);
    assert_eq!(socket_inode("socket:[12x]"), None);
}

#[cfg(target_endian = "little")]
#[test]
fn listening_sockets_are_those_whose_inode_a_member_holds() {
    let held = std::collections::HashSet::from([41_001_u64, 61_001, 41_003]);
    let sockets = sockets_for_inodes(
        &held,
        &[
            (PROC_NET_TCP, SocketProtocol::Tcp),
            (PROC_NET_TCP6, SocketProtocol::Tcp),
            (PROC_NET_UDP, SocketProtocol::Udp),
        ],
    );
    assert_eq!(
        sockets,
        [
            ListeningSocket {
                protocol: SocketProtocol::Tcp,
                address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 8_080,
            },
            ListeningSocket {
                protocol: SocketProtocol::Udp,
                address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: 5_353,
            },
        ]
    );
}

#[test]
fn an_unknown_process_is_not_found() {
    let Some((supervisor, _root, _state)) = delegated("unknown") else {
        return;
    };
    assert_eq!(
        supervisor.observe("hp_absent").expect_err("unknown").code,
        NOT_FOUND
    );
}

// ---------------------------------------------------------------------------------------------
// Delegated cases: absent, never passed, without SUBSTRATE_VECTORS_CGROUP_ROOT.
// ---------------------------------------------------------------------------------------------

/// A private child of the delegated cgroup subtree with the controllers enabled, a state
/// directory, and a supervisor over both — or nothing, and the case makes no claim.
fn delegated(name: &str) -> Option<(Arc<HostProcessSupervisor>, PathBuf, tempfile::TempDir)> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    let path = root.join(format!("host-proc-{name}"));
    if path.is_dir() {
        let _replaced = std::fs::remove_dir(&path);
    }
    std::fs::create_dir(&path).ok()?;
    let controllers = "+cpu +memory +pids";
    std::fs::write(root.join("cgroup.subtree_control"), controllers).ok()?;
    std::fs::write(path.join("cgroup.subtree_control"), controllers).ok()?;
    let state = tempfile::tempdir().ok()?;
    let supervisor = HostProcessSupervisor::open(Some(&path), &state.path().join("processes"))
        .expect("a delegated root serves host processes");
    Some((supervisor, path, state))
}

fn workdir() -> PathBuf {
    std::env::temp_dir().canonicalize().expect("temp dir")
}

fn cgroup_dir(root: &Path, id: &str) -> PathBuf {
    root.join(format!("substrate-hp_{id}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_main_exit_records_its_code_its_tagged_log_and_an_empty_removed_cgroup() {
    let Some((supervisor, root, _state)) = delegated("exit") else {
        return;
    };
    let started = supervisor
        .spawn(
            "hp_exit",
            &spec(
                &[
                    "/bin/sh",
                    "-c",
                    "echo out-line; echo err-line >&2; pwd; exit 3",
                ],
                &workdir(),
            ),
        )
        .expect("spawn");
    assert_eq!(started.profile, HostProfile::Host);
    assert_eq!(started.confinement, Confinement::None);
    let end = supervisor.wait("hp_exit").await.expect("end");
    assert_eq!(end.ended_by, EndedBy::Exit);
    assert_eq!(end.exit, Some(ProcessExit::Code(3)));
    assert!(
        !cgroup_dir(&root, "hp_exit").exists(),
        "the cgroup is removed"
    );

    let tail = supervisor
        .log_tail("hp_exit", LogStreamSelection::Both, 4_096)
        .expect("tail");
    let lines = tail
        .lines
        .iter()
        .map(|line| {
            (
                line.stream,
                String::from_utf8_lossy(&line.bytes).into_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert!(lines.contains(&(LogStream::Stdout, "out-line".to_owned())));
    assert!(lines.contains(&(LogStream::Stderr, "err-line".to_owned())));
    assert!(lines.contains(&(LogStream::Stdout, workdir().display().to_string())));
    supervisor.forget("hp_exit").expect("forget");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_environment_is_exactly_the_request_env() {
    let Some((supervisor, _root, _state)) = delegated("env") else {
        return;
    };
    let mut request = spec(&["/usr/bin/env"], &workdir());
    request.env.insert("ONLY_THIS".to_owned(), "1".to_owned());
    supervisor.spawn("hp_env", &request).expect("spawn");
    supervisor.wait("hp_env").await.expect("end");
    let tail = supervisor
        .log_tail("hp_env", LogStreamSelection::Stdout, 65_536)
        .expect("tail");
    let mut seen = tail
        .lines
        .iter()
        .map(|line| String::from_utf8_lossy(&line.bytes).into_owned())
        .collect::<Vec<_>>();
    seen.sort();
    assert_eq!(seen, ["ONLY_THIS=1", "PATH=/usr/bin:/bin"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_a_missing_program_is_a_named_spawn_failure_and_leaves_no_cgroup() {
    let Some((supervisor, root, _state)) = delegated("missing") else {
        return;
    };
    let error = supervisor
        .spawn("hp_missing", &spec(&["/nonexistent/program"], &workdir()))
        .expect_err("absent program");
    assert_eq!(error.code, super::SPAWN_FAILED);
    assert!(!cgroup_dir(&root, "hp_missing").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_stop_ends_a_cooperative_tree_at_sigterm() {
    let Some((supervisor, root, _state)) = delegated("term") else {
        return;
    };
    supervisor
        .spawn("hp_term", &spec(&["/bin/sleep", "300"], &workdir()))
        .expect("spawn");
    assert!(cgroup_dir(&root, "hp_term").is_dir());
    let end = supervisor
        .stop("hp_term", StopCause::Stop, Duration::from_secs(5))
        .await
        .expect("stop");
    assert_eq!(end.ended_by, EndedBy::Stop);
    assert_eq!(end.stop_step, Some(StopStep::Term));
    assert_eq!(end.exit, Some(ProcessExit::Signal(libc::SIGTERM)));
    assert!(!cgroup_dir(&root, "hp_term").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_stop_kills_a_detached_term_ignoring_tree_with_cgroup_kill() {
    let Some((supervisor, root, _state)) = delegated("kill") else {
        return;
    };
    // The child ignores SIGTERM and leaves a detached grandchild behind, re-parented away
    // from the main process; membership, not the process tree, defines what stop ends.
    let script = "trap '' TERM; setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $! ; sleep 300";
    supervisor
        .spawn("hp_kill", &spec(&["/bin/sh", "-c", script], &workdir()))
        .expect("spawn");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let grandchild = supervisor
        .log_tail("hp_kill", LogStreamSelection::Stdout, 64)
        .expect("tail")
        .lines
        .first()
        .map(|line| String::from_utf8_lossy(&line.bytes).trim().to_owned())
        .expect("the grandchild pid");
    let end = supervisor
        .stop("hp_kill", StopCause::Stop, Duration::from_millis(300))
        .await
        .expect("stop");
    assert_eq!(end.ended_by, EndedBy::Stop);
    assert_eq!(end.stop_step, Some(StopStep::Kill));
    assert!(!cgroup_dir(&root, "hp_kill").exists());
    assert!(
        !Path::new(&format!("/proc/{grandchild}")).exists(),
        "the detached grandchild died with its cgroup"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_main_exit_with_remaining_members_stops_them_and_keeps_exit() {
    let Some((supervisor, root, _state)) = delegated("orphans") else {
        return;
    };
    let script = "setsid sleep 300 </dev/null >/dev/null 2>&1 & exit 0";
    supervisor
        .spawn("hp_orphans", &spec(&["/bin/sh", "-c", script], &workdir()))
        .expect("spawn");
    let end = supervisor.wait("hp_orphans").await.expect("end");
    assert_eq!(end.ended_by, EndedBy::Exit);
    assert_eq!(end.exit, Some(ProcessExit::Code(0)));
    assert!(end.stop_step.is_some(), "the remaining member was stopped");
    assert!(!cgroup_dir(&root, "hp_orphans").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_timeout_ends_the_process_as_timeout_not_stop() {
    let Some((supervisor, root, _state)) = delegated("timeout") else {
        return;
    };
    let mut request = spec(&["/bin/sleep", "300"], &workdir());
    request.deadline = Deadline::effective(Some(1_000), None, Utc::now()).expect("deadline");
    supervisor.spawn("hp_timeout", &request).expect("spawn");
    let end = tokio::time::timeout(Duration::from_secs(20), supervisor.wait("hp_timeout"))
        .await
        .expect("the deadline fires")
        .expect("end");
    assert_eq!(end.ended_by, EndedBy::Timeout);
    assert!(!cgroup_dir(&root, "hp_timeout").exists());
    // A later stop does not overwrite the first recorded cause.
    let again = supervisor
        .stop("hp_timeout", StopCause::Stop, Duration::from_secs(1))
        .await
        .expect("stop after end");
    assert_eq!(again.ended_by, EndedBy::Timeout);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_expires_at_ends_the_process_as_expires_at() {
    let Some((supervisor, _root, _state)) = delegated("expires") else {
        return;
    };
    let at = (Utc::now() + TimeDelta::milliseconds(1_200))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut request = spec(&["/bin/sleep", "300"], &workdir());
    request.deadline = Deadline::effective(None, Some(&at), Utc::now()).expect("deadline");
    supervisor.spawn("hp_expires", &request).expect("spawn");
    let end = tokio::time::timeout(Duration::from_secs(20), supervisor.wait("hp_expires"))
        .await
        .expect("the deadline fires")
        .expect("end");
    assert_eq!(end.ended_by, EndedBy::ExpiresAt);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_lease_expiry_stop_is_recorded_as_lease_expiry() {
    let Some((supervisor, _root, _state)) = delegated("lease") else {
        return;
    };
    supervisor
        .spawn("hp_lease", &spec(&["/bin/sleep", "300"], &workdir()))
        .expect("spawn");
    let end = supervisor
        .stop("hp_lease", StopCause::LeaseExpiry, Duration::from_secs(5))
        .await
        .expect("stop");
    assert_eq!(end.ended_by, EndedBy::LeaseExpiry);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_observation_reads_cpu_memory_and_a_listening_port_a_member_holds() {
    let Some((supervisor, _root, _state)) = delegated("observe") else {
        return;
    };
    // The member is this test binary itself, running only `listening_helper`, which binds a
    // loopback port, burns a little CPU, prints the port and sleeps. No tool outside the
    // repository is needed, and the driver's own descriptor hygiene stays in force.
    let exe = std::env::current_exe().expect("test binary");
    let exe = exe.to_str().expect("utf-8 test binary path");
    let mut request = spec(
        &[
            exe,
            "--exact",
            "host_process::tests::listening_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ],
        &workdir(),
    );
    request
        .env
        .insert(LISTENING_HELPER_ENV.to_owned(), "1".to_owned());
    supervisor.spawn("hp_observe", &request).expect("spawn");
    let mut port = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        port = supervisor
            .log_tail("hp_observe", LogStreamSelection::Stdout, 65_536)
            .expect("tail")
            .lines
            .iter()
            .find_map(|line| {
                std::str::from_utf8(&line.bytes)
                    .ok()?
                    .strip_prefix("LISTENING ")?
                    .trim()
                    .parse::<u16>()
                    .ok()
            });
        if port.is_some() {
            break;
        }
    }
    let port = port.expect("the helper reported its port");
    let observed = supervisor.observe("hp_observe").expect("observe");
    assert_eq!(observed.confinement, Confinement::None);
    assert!(observed.cpu_usec.expect("cpu observed") > 0);
    assert!(observed.memory_bytes.expect("memory observed") > 0);
    let listening = observed.listening.expect("sockets observed");
    assert!(
        listening.iter().any(|socket| socket.port == port
            && socket.protocol == SocketProtocol::Tcp
            && socket.address == IpAddr::V4(Ipv4Addr::LOCALHOST)),
        "port {port} in {listening:?}"
    );
    supervisor
        .stop("hp_observe", StopCause::Stop, Duration::from_secs(5))
        .await
        .expect("stop");
    let after = supervisor.observe("hp_observe").expect("an ended record");
    assert!(after.cpu_usec.is_none() && after.memory_bytes.is_none() && after.listening.is_none());
}

const LISTENING_HELPER_ENV: &str = "SUBSTRATE_HOST_PROCESS_LISTENING_HELPER";

/// Not a case: the program the observation case runs as a host process. Ignored so that no lane
/// counts it as a pass; it does nothing unless that case's environment variable is set.
#[test]
#[ignore = "the observation case's child program, never a check of its own"]
fn listening_helper() {
    if std::env::var_os(LISTENING_HELPER_ENV).is_none() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let mut spin = 0_u64;
    for index in 0..20_000_000_u64 {
        spin = spin.wrapping_add(std::hint::black_box(index));
    }
    std::hint::black_box(spin);
    println!(
        "LISTENING {}",
        listener.local_addr().expect("address").port()
    );
    std::thread::sleep(Duration::from_mins(5));
}
