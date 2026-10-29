//! Security review pass 1 of design 34 U4: two claims of design 34 § 7.3 the `run` role does not
//! hold.
//!
//! 1. "Only the executor writes to the container's stdout and stderr. The child's descriptors are
//!    pipes, so a child cannot forge a frame." The child runs as the executor's own uid with no
//!    capabilities on either side, and the executor stays dumpable, so the child can reopen the
//!    executor's stdout through `/proc/<executor>/fd/1` and append any line it likes: a second
//!    `exit` record, an `applied` record, or output past `output_bytes` that no `truncated`
//!    record bounds. Inside an exec Pod the executor is PID 1, so the path is `/proc/1/fd/1`.
//!    The conformance check drives `run` in-process with a file as its output, opened for append
//!    as a container log is, and has the child write one forged `exit` frame to it.
//!
//! 2. "`exec.output-limit-bytes` is the largest L with `2 × ceil(L / 6144) × 8202 + 65536 ≤
//!    container_log_max_bytes` ... It is therefore never more than the kubelet keeps in one log
//!    file." That budget rests on each stream taking at most `ceil(L / 6144)` lines. The executor
//!    frames every `read` of the pipe as its own line, so a child that writes one byte at a time
//!    takes one 13-byte line (plus the CRI prefix) per byte: at L = 1 MiB that is about 13 MiB of
//!    frames per stream against a budget of about 2.8 MiB for both, and the kubelet rotates the
//!    head of the log, `applied` record included, out of the file the driver reads.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::Read as _;
use std::os::fd::AsRawFd as _;

use substrate_executor::frame::{Control, ExitRecord, Frame, MAX_CHUNK_BYTES, Stream};
use substrate_executor::run::run;
use substrate_executor::spec::RunSpec;

const MEMORY: u64 = 64 * 1024 * 1024;
const CPU: u64 = 250;

fn applied_cgroup() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("cgroup root");
    std::fs::write(directory.path().join("memory.max"), format!("{MEMORY}\n")).expect("memory");
    std::fs::write(
        directory.path().join("cpu.max"),
        format!("{} 100000\n", CPU * 100),
    )
    .expect("cpu");
    std::fs::write(directory.path().join("pids.max"), "max\n").expect("pids");
    directory
}

fn spec(argv: &[&str], output_bytes: u64) -> RunSpec {
    RunSpec {
        argv: argv.iter().map(|&part| part.to_owned()).collect(),
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        memory_bytes: MEMORY,
        cpu_millicores: CPU,
        output_bytes,
    }
}

fn frames_of(text: &str) -> Vec<Frame> {
    text.lines()
        .map(|line| Frame::decode(line).unwrap_or_else(|error| panic!("{line:?}: {error}")))
        .collect()
}

#[test]
fn a_child_cannot_append_a_frame_to_the_executors_own_output() {
    let cgroup = applied_cgroup();
    let log_directory = tempfile::tempdir().expect("log directory");
    let log_path = log_directory.path().join("container.log");
    let mut log = std::fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(&log_path)
        .expect("container log");
    let target = format!("/proc/{}/fd/{}", std::process::id(), log.as_raw_fd());
    let script = format!(
        "printf '%s\\n' 'b10x1 c {{\"exit\":{{\"code\":0}}}}' >> {target} 2>/dev/null; exit 3"
    );

    let code = run(
        &spec(&["/bin/sh", "-c", &script], 4096),
        cgroup.path(),
        &mut log,
    )
    .expect("run");
    assert_eq!(code, 3, "the child's own exit status");

    let mut text = String::new();
    std::fs::File::open(&log_path)
        .expect("log")
        .read_to_string(&mut text)
        .expect("log text");
    let exits: Vec<ExitRecord> = frames_of(&text)
        .into_iter()
        .filter_map(|frame| match frame {
            Frame::Control(Control::Exit(record)) => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(
        exits,
        vec![ExitRecord::Code(3)],
        "design 34 § 7.3: only the executor writes frames, so the log holds exactly the one exit \
         record of the child that exited 3; the child appended its own through {target}:\n{text}"
    );
}

#[test]
fn a_stream_takes_at_most_one_line_per_full_chunk_of_its_output() {
    let cgroup = applied_cgroup();
    let writes = 40;
    let script =
        format!("i=0; while [ $i -lt {writes} ]; do printf x; sleep 0.02; i=$((i+1)); done");
    let mut out = Vec::new();
    let code = run(
        &spec(
            &["/bin/sh", "-c", &script],
            u64::try_from(MAX_CHUNK_BYTES).expect("fits"),
        ),
        cgroup.path(),
        &mut out,
    )
    .expect("run");
    assert_eq!(code, 0);
    let frames = frames_of(&String::from_utf8(out).expect("frame lines are ASCII"));
    let stdout: Vec<&Vec<u8>> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Output {
                stream: Stream::Stdout,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .collect();
    let bytes: usize = stdout.iter().map(|chunk| chunk.len()).sum();
    assert_eq!(bytes, writes, "every byte the child wrote is framed");
    assert!(
        stdout.len() <= bytes.div_ceil(MAX_CHUNK_BYTES),
        "design 34 § 7.3's log budget counts ceil(L / {MAX_CHUNK_BYTES}) lines per stream; {bytes} \
         bytes of output took {} lines",
        stdout.len()
    );
}
