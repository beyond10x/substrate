//! Design 34 § 7.3 step 4: a `TERM` that reaches the executor before the child's process group is
//! known is forwarded once it is, not dropped. The case follows the binary's order: `TERM` blocked
//! and one pending, then `forward_term` installs the handler, which records it, and `run` forwards
//! it to the child's group once the group is stored. Its own test binary, because the handler is
//! process-wide.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use substrate_executor::frame::{Control, ExitRecord, Frame};
use substrate_executor::run::{forward_term, run};
use substrate_executor::spec::RunSpec;

const MEMORY: u64 = 64 * 1024 * 1024;
const CPU: u64 = 250;

#[test]
fn a_term_pending_before_the_spawn_reaches_the_child() {
    let cgroup = tempfile::tempdir().expect("cgroup root");
    std::fs::write(cgroup.path().join("memory.max"), format!("{MEMORY}\n")).expect("memory");
    std::fs::write(
        cgroup.path().join("cpu.max"),
        format!("{} 100000\n", CPU * 100),
    )
    .expect("cpu");
    let spec = RunSpec {
        argv: ["/bin/sleep", "5"]
            .iter()
            .map(|&part| part.to_owned())
            .collect(),
        env: BTreeMap::new(),
        memory_bytes: MEMORY,
        cpu_millicores: CPU,
        output_bytes: 4096,
    };
    // SAFETY: blocks TERM on this thread only, then makes one TERM pending on this thread.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()),
            0
        );
        assert_eq!(libc::pthread_kill(libc::pthread_self(), libc::SIGTERM), 0);
    }
    forward_term();

    let started = Instant::now();
    let mut out = Vec::new();
    let code = run(&spec, cgroup.path(), &mut out).expect("run");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the TERM pending before the spawn was never forwarded; the child ran its full 5 s"
    );
    assert_eq!(code, 128 + libc::SIGTERM);
    let last = String::from_utf8(out)
        .expect("ASCII")
        .lines()
        .map(|line| Frame::decode(line).expect("frame"))
        .next_back();
    assert_eq!(
        last,
        Some(Frame::Control(Control::Exit(ExitRecord::Signal(
            "term".to_owned()
        ))))
    );
}
