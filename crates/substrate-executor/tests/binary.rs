//! The shipped `substrate-executor` binary's roles (design 34 § 7).

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use substrate_executor::Handshake;
use substrate_executor::frame::{Control, Frame, Refusal};
use substrate_executor::spec::RunSpec;

const EXECUTOR: &str = env!("CARGO_BIN_EXE_substrate-executor");

#[test]
fn version_writes_the_handshake_line_and_nothing_else() {
    let output = Command::new(EXECUTOR)
        .arg("version")
        .output()
        .expect("executor");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    let line = stdout.strip_suffix('\n').expect("one line");
    assert!(!line.contains('\n'));
    let handshake = Handshake::parse(line).expect("handshake");
    assert!(handshake.is_current());
    assert_eq!(handshake, Handshake::current());
    assert!(output.stderr.is_empty());
}

#[test]
fn probe_child_writes_its_whole_argv_as_one_json_line_and_exits_0() {
    let output = Command::new(EXECUTOR)
        .arg(substrate_executor::probe::PROBE_CHILD)
        .args(substrate_executor::probe::PROBE_ARGUMENTS)
        .env_clear()
        .output()
        .expect("executor");
    assert_eq!(output.status.code(), Some(0));
    let mut argv = vec![EXECUTOR, substrate_executor::probe::PROBE_CHILD];
    argv.extend(substrate_executor::probe::PROBE_ARGUMENTS);
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf-8"),
        substrate_executor::probe::report(&argv)
    );
    assert!(output.stderr.is_empty());
}

/// The pid and beat the probe grandchild's heartbeat file holds.
fn heartbeat(path: &Path) -> Option<(libc::pid_t, u64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (pid, beat) = text.strip_suffix('\n')?.split_once(' ')?;
    Some((pid.parse().ok()?, beat.parse().ok()?))
}

/// Kills the probe grandchild when the case ends, passed or not: outside a PID namespace nothing
/// else ever does.
struct Grandchild(libc::pid_t);

impl Drop for Grandchild {
    fn drop(&mut self) {
        // SAFETY: kill takes integers only; the pid is the grandchild this case's child forked.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
        }
    }
}

/// `probe-child` with targets, as the probe Pod runs it, its stdout read to the end on a thread
/// so a descendant that kept the pipe open fails the case instead of hanging it.
fn probe_child_with(targets: &str) -> (Option<i32>, String, Vec<u8>) {
    let mut child = Command::new(EXECUTOR)
        .arg(substrate_executor::probe::PROBE_CHILD)
        .args(substrate_executor::probe::PROBE_ARGUMENTS)
        .env_clear()
        .env(substrate_executor::probe::TARGETS_ENV, targets)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("executor");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (sender, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let mut err = Vec::new();
        let read = std::io::Read::read_to_string(&mut stdout, &mut out)
            .and_then(|_| std::io::Read::read_to_end(&mut stderr, &mut err));
        let _ = sender.send(read.map(|_| (out, err)));
    });
    let status = child.wait().expect("the child ends");
    let (stdout, stderr) = received
        .recv_timeout(Duration::from_secs(10))
        .expect("no descendant of the child holds its stdout or stderr open")
        .expect("both streams read");
    (status.code(), stdout, stderr)
}

#[test]
fn probe_child_with_targets_writes_its_argv_line_then_what_it_observed() {
    use substrate_executor::probe::{
        EgressOutcome, EgressTarget, HEARTBEAT_FILE, Observation, PROBE_ARGUMENTS, PROBE_CHILD,
        ProbeTargets, report,
    };

    let volume = tempfile::tempdir().expect("the probe's volume");
    let beating = volume.path().join(HEARTBEAT_FILE);
    let resolv = volume.path().join("resolv.conf");
    std::fs::write(&resolv, "search cluster.local\n").expect("a resolver without a nameserver");
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("an egress listener");
    let egress = listener.local_addr().expect("its address");
    let targets = ProbeTargets {
        heartbeat: beating.to_str().expect("utf-8").to_owned(),
        kubernetes_service_host: None,
        resolv_conf: resolv.to_str().expect("utf-8").to_owned(),
        egress_probe_address: egress,
    };
    let (code, stdout, stderr) = probe_child_with(&targets.encode());
    let grandchild = heartbeat(&beating).map(|(pid, _)| Grandchild(pid));
    assert_eq!(code, Some(0));
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));

    let mut argv = vec![EXECUTOR, PROBE_CHILD];
    argv.extend(PROBE_ARGUMENTS);
    let argv_line = report(&argv);
    let observed = stdout
        .strip_prefix(&argv_line)
        .unwrap_or_else(|| panic!("the argv line keeps its bytes and comes first: {stdout:?}"));
    let observation = Observation::parse_line(observed)
        .unwrap_or_else(|| panic!("one observation line follows: {observed:?}"));
    assert!(
        observation.heartbeat,
        "the grandchild's heartbeat was seen to move"
    );
    let seen: Vec<_> = observation
        .egress
        .iter()
        .map(|attempt| (attempt.target, attempt.address, attempt.outcome))
        .collect();
    assert_eq!(
        seen,
        [
            (EgressTarget::KubernetesService, None, EgressOutcome::Unread),
            (EgressTarget::Nameserver, None, EgressOutcome::Unread),
            (
                EgressTarget::EgressProbe,
                Some(egress),
                EgressOutcome::Reached
            ),
        ]
    );

    // Outside a PID namespace nothing kills the grandchild: it beats on after its parent ended,
    // in a session of its own, through TERM.
    let grandchild = grandchild.expect("the heartbeat names the grandchild");
    let (pid, before) = heartbeat(&beating).expect("a heartbeat");
    assert_eq!(pid, grandchild.0);
    // SAFETY: getsid and kill take integers only; the pid is this case's grandchild.
    assert_eq!(
        unsafe { libc::getsid(pid) },
        pid,
        "it leads its own session"
    );
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    std::thread::sleep(Duration::from_millis(500));
    let (_, after) = heartbeat(&beating).expect("a heartbeat after TERM");
    assert!(
        after > before,
        "the heartbeat moves past TERM: {before} -> {after}"
    );
}

#[test]
fn probe_child_with_targets_it_cannot_read_writes_only_its_argv_line() {
    use substrate_executor::probe::{PROBE_ARGUMENTS, PROBE_CHILD, report};

    let (code, stdout, stderr) = probe_child_with("{\"shell\":\"/bin/sh\"}");
    assert_eq!(code, Some(0));
    assert!(stderr.is_empty());
    let mut argv = vec![EXECUTOR, PROBE_CHILD];
    argv.extend(PROBE_ARGUMENTS);
    assert_eq!(
        stdout,
        report(&argv),
        "nothing is observed for unreadable targets"
    );
}

#[test]
fn an_unknown_or_missing_subcommand_exits_2() {
    for arguments in [
        vec![],
        vec!["shell"],
        vec!["version", "extra"],
        vec!["install"],
    ] {
        let status = Command::new(EXECUTOR)
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("executor");
        assert_eq!(status.code(), Some(2), "{arguments:?}");
    }
}

#[test]
fn run_with_an_undecodable_spec_writes_a_refusal_frame_and_exits_2() {
    let output = Command::new(EXECUTOR)
        .args(["run", "not-a-spec"])
        .output()
        .expect("executor");
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    let frames: Vec<Frame> = stdout
        .lines()
        .map(|line| Frame::decode(line).expect("frame"))
        .collect();
    assert_eq!(
        frames,
        [Frame::Control(Control::Refused(Refusal::SpecInvalid))]
    );
}

#[test]
fn run_outside_an_exec_pod_reads_no_matching_bounds_and_refuses() {
    // The binary always reads `/sys/fs/cgroup`; on a host that is not a container whose bounds are
    // exactly these, the spec's limits are not applied and the argv must not start.
    let marker = tempfile::tempdir().expect("marker");
    let touched = marker.path().join("started");
    let spec = RunSpec {
        argv: vec![
            "/usr/bin/touch".to_owned(),
            touched.to_str().expect("utf-8").to_owned(),
        ],
        env: std::collections::BTreeMap::new(),
        memory_bytes: 12_345_678,
        cpu_millicores: 7,
        output_bytes: 1024,
    };
    let output = Command::new(EXECUTOR)
        .arg("run")
        .args(spec.encode_parts())
        .output()
        .expect("executor");
    assert_eq!(output.status.code(), Some(125));
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    let last = stdout.lines().last().expect("a frame");
    assert_eq!(
        Frame::decode(last).expect("frame"),
        Frame::Control(Control::Refused(Refusal::LimitsUnapplied))
    );
    assert!(!touched.exists());
}

#[test]
fn install_copies_the_executor_and_refuses_an_existing_path() {
    let directory = tempfile::tempdir().expect("install directory");
    let target = directory.path().join("substrate-executor");
    let status = Command::new(EXECUTOR)
        .args(["install", target.to_str().expect("utf-8")])
        .status()
        .expect("executor");
    assert!(status.success());
    assert_eq!(
        std::fs::read(&target).expect("installed"),
        std::fs::read(EXECUTOR).expect("original")
    );
    let output = Command::new(&target)
        .arg("version")
        .output()
        .expect("installed executor");
    assert!(output.status.success());

    let again = Command::new(EXECUTOR)
        .args(["install", target.to_str().expect("utf-8")])
        .stderr(Stdio::null())
        .status()
        .expect("executor");
    assert_eq!(again.code(), Some(1));
}

#[test]
fn keeper_exits_on_term() {
    let mut keeper = Command::new(EXECUTOR)
        .arg("keeper")
        .stdin(Stdio::null())
        .spawn()
        .expect("keeper");
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        keeper.try_wait().expect("poll").is_none(),
        "keeper stays up"
    );
    // SAFETY: the pid is the keeper this test spawned and still holds.
    let pid = libc::pid_t::try_from(keeper.id()).expect("pid");
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = keeper.try_wait().expect("poll") {
            assert_eq!(status.code(), Some(0));
            break;
        }
        assert!(Instant::now() < deadline, "keeper did not exit on TERM");
        std::thread::sleep(Duration::from_millis(20));
    }
}
