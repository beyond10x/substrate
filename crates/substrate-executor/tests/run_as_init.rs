//! Design 34 § 7.3: as an exec Pod's PID 1, the executor reaps every orphan reparented to it, not
//! only its direct child, so an exited grandchild does not stay a zombie holding a `pids.max`
//! slot. Its own test binary, because `run_as_init` reaps every child of the process it runs in.
//!
//! Outside a PID namespace an orphan reparents to the nearest subreaper, so the case asserts both
//! halves: the orphan's parent is this process, and once it exits nothing of it remains.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;

use substrate_executor::frame::{Control, ExitRecord, Frame, Stream};
use substrate_executor::run::run_as_init;
use substrate_executor::spec::RunSpec;

const MEMORY: u64 = 64 * 1024 * 1024;
const CPU: u64 = 250;

#[test]
fn an_orphaned_grandchild_is_reparented_to_the_executor_and_reaped() {
    let cgroup = tempfile::tempdir().expect("cgroup root");
    std::fs::write(cgroup.path().join("memory.max"), format!("{MEMORY}\n")).expect("memory");
    std::fs::write(
        cgroup.path().join("cpu.max"),
        format!("{} 100000\n", CPU * 100),
    )
    .expect("cpu");
    let script = "p=$(sh -c 'sleep 0.3 >/dev/null 2>&1 & echo $!'); \
                  sed -n 's/^PPid:[[:space:]]*//p' /proc/$p/status; sleep 0.9; \
                  if [ -e /proc/$p ]; then echo left; else echo gone; fi";
    let spec = RunSpec {
        argv: ["/bin/sh", "-c", script]
            .iter()
            .map(|&part| part.to_owned())
            .collect(),
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        memory_bytes: MEMORY,
        cpu_millicores: CPU,
        output_bytes: 4096,
    };
    let mut out = Vec::new();
    let code = run_as_init(&spec, cgroup.path(), &mut out).expect("run");
    assert_eq!(code, 0);
    let frames: Vec<Frame> = String::from_utf8(out)
        .expect("ASCII")
        .lines()
        .map(|line| Frame::decode(line).expect("frame"))
        .collect();
    let stdout: Vec<u8> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Output {
                stream: Stream::Stdout,
                bytes,
            } => Some(bytes.as_slice()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect();
    let stdout = String::from_utf8(stdout).expect("text");
    assert_eq!(
        stdout,
        format!("{}\ngone\n", std::process::id()),
        "the orphan's parent is the executor, and it was reaped once it exited"
    );
    assert_eq!(
        frames.last(),
        Some(&Frame::Control(Control::Exit(ExitRecord::Code(0))))
    );
}
