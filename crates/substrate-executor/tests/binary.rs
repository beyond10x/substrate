//! The shipped `substrate-executor` binary's roles (design 34 § 7).

#![cfg(target_os = "linux")]

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
