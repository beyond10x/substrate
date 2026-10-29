//! Security review pass 1 of design 34 U4: design 34 § 7.3 step 4, "On `TERM` it forwards `TERM`
//! to the child's process group", had no case. Its own test binary, because the handler
//! [`forward_term`] installs is process-wide and the group it forwards to is one global.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use substrate_executor::frame::{Control, ExitRecord, Frame, Stream};
use substrate_executor::run::{forward_term, run};
use substrate_executor::spec::RunSpec;

const MEMORY: u64 = 64 * 1024 * 1024;
const CPU: u64 = 250;

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn frames(log: &Shared) -> Vec<Frame> {
    let text = String::from_utf8(log.0.lock().expect("log").clone()).expect("ASCII");
    text.lines()
        .map(|line| Frame::decode(line).expect("frame"))
        .collect()
}

#[test]
fn term_to_the_executor_reaches_the_childs_whole_process_group() {
    let cgroup = tempfile::tempdir().expect("cgroup root");
    std::fs::write(cgroup.path().join("memory.max"), format!("{MEMORY}\n")).expect("memory");
    std::fs::write(
        cgroup.path().join("cpu.max"),
        format!("{} 100000\n", CPU * 100),
    )
    .expect("cpu");
    let spec = RunSpec {
        argv: ["/bin/sh", "-c", "sleep 30 & printf '%-6144s' $!; wait"]
            .iter()
            .map(|&part| part.to_owned())
            .collect(),
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        memory_bytes: MEMORY,
        cpu_millicores: CPU,
        output_bytes: 4096,
    };
    forward_term();
    let log = Shared::default();
    let mut writer = log.clone();
    let root = cgroup.path().to_path_buf();
    let runner = std::thread::spawn(move || run(&spec, &root, &mut writer).expect("run"));

    let started = Instant::now();
    let grandchild: libc::pid_t = loop {
        let printed = frames(&log).into_iter().find_map(|frame| match frame {
            Frame::Output {
                stream: Stream::Stdout,
                bytes,
            } => String::from_utf8(bytes).ok()?.trim_end().parse().ok(),
            _ => None,
        });
        if let Some(pid) = printed {
            break pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the child never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    // SAFETY: signals this test process, whose TERM handler forward_term installed above.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGTERM);
    }
    let code = runner.join().expect("runner");
    assert_eq!(code, 128 + libc::SIGTERM);
    let gone = Instant::now();
    // A zombie has ended; whoever it was reparented to may reap it late or never.
    let running = || {
        std::fs::read_to_string(format!("/proc/{grandchild}/stat"))
            .ok()
            .and_then(|stat| {
                let state = stat.rsplit_once(") ")?.1.chars().next()?;
                Some(state != 'Z' && state != 'X')
            })
            .unwrap_or(false)
    };
    while running() {
        assert!(
            gone.elapsed() < Duration::from_secs(5),
            "the background sleep {grandchild} in the child's group outlived the forwarded TERM"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let last = frames(&log).pop();
    assert_eq!(
        last,
        Some(Frame::Control(Control::Exit(ExitRecord::Signal(
            "term".to_owned()
        ))))
    );
}
