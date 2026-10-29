//! Design 34 § 7.3: the exec Pod's PID 1, driven in-process with a test cgroup root.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::Path;

use substrate_executor::frame::{Control, ExitRecord, Frame, Refusal, Stream};
use substrate_executor::run::{LIMITS_UNAPPLIED_EXIT, run};
use substrate_executor::spec::{MAX_PART_BYTES, RunSpec};

const MEMORY: u64 = 64 * 1024 * 1024;
const CPU: u64 = 250;

/// A cgroup root that reads back exactly what kubelet writes for `MEMORY` and `CPU`.
fn cgroup(memory: &str, cpu: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("cgroup root");
    std::fs::write(directory.path().join("memory.max"), memory).expect("memory.max");
    std::fs::write(directory.path().join("cpu.max"), cpu).expect("cpu.max");
    std::fs::write(directory.path().join("pids.max"), "max\n").expect("pids.max");
    directory
}

fn applied_cgroup() -> tempfile::TempDir {
    cgroup(&format!("{MEMORY}\n"), &format!("{} 100000\n", CPU * 100))
}

fn spec(argv: &[&str], env: &[(&str, &str)], output_bytes: u64) -> RunSpec {
    RunSpec {
        argv: argv.iter().map(|&part| part.to_owned()).collect(),
        env: env
            .iter()
            .map(|&(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<BTreeMap<_, _>>(),
        memory_bytes: MEMORY,
        cpu_millicores: CPU,
        output_bytes,
    }
}

struct Ran {
    code: i32,
    frames: Vec<Frame>,
}

impl Ran {
    fn stream(&self, wanted: Stream) -> Vec<u8> {
        self.frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::Output { stream, bytes } if *stream == wanted => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect()
    }

    fn controls(&self) -> Vec<&Control> {
        self.frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::Control(control) => Some(control),
                Frame::Output { .. } => None,
            })
            .collect()
    }
}

fn run_in(spec: &RunSpec, cgroup_root: &Path) -> Ran {
    let mut out = Vec::new();
    let code = run(spec, cgroup_root, &mut out).expect("run");
    let text = String::from_utf8(out).expect("frame lines are ASCII");
    assert!(text.is_empty() || text.ends_with('\n'));
    let frames = text
        .lines()
        .map(|line| Frame::decode(line).unwrap_or_else(|error| panic!("{line:?}: {error}")))
        .collect();
    Ran { code, frames }
}

#[test]
fn argv_reaches_the_child_byte_for_byte_without_a_shell() {
    let cgroup = applied_cgroup();
    let argv = [
        "/usr/bin/printf",
        "[%s]",
        "$(HOME)",
        "$HOME",
        "${PATH}",
        "a b",
        "*",
        "'quoted'",
        "`id`",
        "",
        "ünï",
    ];
    let ran = run_in(&spec(&argv, &[], 4096), cgroup.path());
    assert_eq!(ran.code, 0);
    assert_eq!(
        String::from_utf8(ran.stream(Stream::Stdout)).expect("utf-8"),
        "[$(HOME)][$HOME][${PATH}][a b][*]['quoted'][`id`][][ünï]"
    );
    assert!(ran.stream(Stream::Stderr).is_empty());
    assert_eq!(
        ran.controls().last(),
        Some(&&Control::Exit(ExitRecord::Code(0)))
    );
}

#[test]
fn the_child_environment_is_exactly_the_spec() {
    // The test process carries cargo's whole environment; none of it may reach the child.
    assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
    let cgroup = applied_cgroup();
    let ran = run_in(
        &spec(
            &["/usr/bin/env"],
            &[
                ("LANG", "C.UTF-8"),
                ("PATH", "/usr/bin:/bin"),
                ("VALUE", "$(HOME) and $HOME"),
            ],
            4096,
        ),
        cgroup.path(),
    );
    assert_eq!(ran.code, 0);
    assert_eq!(
        String::from_utf8(ran.stream(Stream::Stdout)).expect("utf-8"),
        "LANG=C.UTF-8\nPATH=/usr/bin:/bin\nVALUE=$(HOME) and $HOME\n"
    );

    let empty = run_in(&spec(&["/usr/bin/env"], &[], 4096), cgroup.path());
    assert_eq!(empty.code, 0);
    assert!(empty.stream(Stream::Stdout).is_empty());
}

#[test]
fn interleaved_streams_are_each_tagged_with_their_own_stream() {
    let cgroup = applied_cgroup();
    let ran = run_in(
        &spec(
            &[
                "/bin/sh",
                "-c",
                "printf o1; printf e1 >&2; printf o2; printf e2 >&2; exit 3",
            ],
            &[],
            4096,
        ),
        cgroup.path(),
    );
    assert_eq!(ran.code, 3);
    assert_eq!(ran.stream(Stream::Stdout), b"o1o2");
    assert_eq!(ran.stream(Stream::Stderr), b"e1e2");
    let controls = ran.controls();
    assert!(matches!(controls.first(), Some(Control::Applied(_))));
    assert_eq!(controls.last(), Some(&&Control::Exit(ExitRecord::Code(3))));
    assert!(matches!(
        ran.frames.last(),
        Some(Frame::Control(Control::Exit(_)))
    ));
}

#[test]
fn output_past_the_bound_is_truncated_once_per_stream_and_the_rest_discarded() {
    let cgroup = applied_cgroup();
    let ran = run_in(
        &spec(
            &[
                "/bin/sh",
                "-c",
                "head -c 20000 /dev/zero; printf abcdefghij >&2; printf klmnop >&2",
            ],
            &[],
            8,
        ),
        cgroup.path(),
    );
    assert_eq!(ran.code, 0);
    assert_eq!(ran.stream(Stream::Stdout), vec![0_u8; 8]);
    assert_eq!(ran.stream(Stream::Stderr), b"abcdefgh");
    let truncations: Vec<&Control> = ran
        .controls()
        .into_iter()
        .filter(|control| matches!(control, Control::Truncated(_)))
        .collect();
    assert_eq!(truncations.len(), 2);
    assert!(truncations.contains(&&Control::Truncated(Stream::Stdout)));
    assert!(truncations.contains(&&Control::Truncated(Stream::Stderr)));
    for frame in &ran.frames {
        if let Frame::Output { bytes, .. } = frame {
            assert!(bytes.len() <= substrate_executor::frame::MAX_CHUNK_BYTES);
        }
    }

    let exact = run_in(
        &spec(&["/usr/bin/printf", "12345678"], &[], 8),
        cgroup.path(),
    );
    assert_eq!(exact.stream(Stream::Stdout), b"12345678");
    assert!(
        !exact
            .controls()
            .iter()
            .any(|control| matches!(control, Control::Truncated(_))),
        "output exactly at the bound is not truncated"
    );
}

#[test]
fn limits_that_differ_from_the_spec_refuse_before_the_argv_starts() {
    let marker = tempfile::tempdir().expect("marker directory");
    let touched = marker.path().join("started");
    let argv = ["/usr/bin/touch", touched.to_str().expect("utf-8")];
    let cases = [
        cgroup("max\n", &format!("{} 100000\n", CPU * 100)),
        cgroup(
            &format!("{}\n", MEMORY + 4096),
            &format!("{} 100000\n", CPU * 100),
        ),
        cgroup(&format!("{MEMORY}\n"), "max 100000\n"),
        cgroup(&format!("{MEMORY}\n"), &format!("{} 100000\n", CPU * 200)),
        cgroup("garbage", &format!("{} 100000\n", CPU * 100)),
    ];
    for root in &cases {
        let ran = run_in(&spec(&argv, &[], 4096), root.path());
        assert_eq!(ran.code, LIMITS_UNAPPLIED_EXIT);
        assert_eq!(LIMITS_UNAPPLIED_EXIT, 125);
        let controls = ran.controls();
        assert!(matches!(controls.first(), Some(Control::Applied(_))));
        assert_eq!(
            controls.last(),
            Some(&&Control::Refused(Refusal::LimitsUnapplied))
        );
        assert!(!touched.exists(), "the argv must not have started");
    }

    let absent = tempfile::tempdir().expect("empty cgroup root");
    let ran = run_in(&spec(&argv, &[], 4096), absent.path());
    assert_eq!(ran.code, LIMITS_UNAPPLIED_EXIT);
    assert_eq!(
        ran.controls().first(),
        Some(&&Control::Applied(substrate_executor::frame::Applied {
            memory_bytes: None,
            cpu_millicores: None,
            pids_max: None,
        }))
    );
    assert!(!touched.exists());

    let applied = applied_cgroup();
    let ran = run_in(&spec(&argv, &[], 4096), applied.path());
    assert_eq!(ran.code, 0);
    assert_eq!(
        ran.controls().first(),
        Some(&&Control::Applied(substrate_executor::frame::Applied {
            memory_bytes: Some(MEMORY),
            cpu_millicores: Some(CPU),
            pids_max: Some("max".to_owned()),
        }))
    );
    assert!(touched.exists(), "matching limits start the argv");
}

#[test]
fn a_signalled_child_is_recorded_by_signal_and_exits_128_plus_it() {
    let cgroup = applied_cgroup();
    let ran = run_in(
        &spec(&["/bin/sh", "-c", "kill -TERM $$"], &[], 4096),
        cgroup.path(),
    );
    assert_eq!(ran.code, 128 + 15);
    assert_eq!(
        ran.controls().last(),
        Some(&&Control::Exit(ExitRecord::Signal("term".to_owned())))
    );
}

/// A frame writer that blocks every output frame while `held` is set, as a container stdout the
/// runtime does not read blocks.
struct Held {
    held: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl std::io::Write for Held {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.starts_with(b"b10x1 o") || bytes.starts_with(b"b10x1 e") {
            while self.held.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_blocked_frame_writer_holds_the_child_on_its_pipes_instead_of_queueing_its_output() {
    let cgroup = applied_cgroup();
    let scratch = tempfile::tempdir().expect("scratch");
    let marker = scratch.path().join("wrote-everything");
    let script = format!(
        "head -c 4000000 /dev/zero; : > {}",
        marker.to_str().expect("UTF-8 path")
    );
    let spec = spec(
        &["/bin/sh", "-c", &script],
        &[("PATH", "/usr/bin:/bin")],
        65536,
    );
    let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut writer = Held {
        held: std::sync::Arc::clone(&held),
    };
    let root = cgroup.path().to_path_buf();
    let runner = std::thread::spawn(move || run(&spec, &root, &mut writer).expect("run"));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let finished_while_held = marker.exists();
    held.store(false, std::sync::atomic::Ordering::SeqCst);
    let code = runner.join().expect("runner");
    assert!(
        !finished_while_held,
        "the child wrote all 4 MB while no frame could be written: its output was queued in the \
         executor's memory instead of holding it on its pipes"
    );
    assert_eq!(code, 0);
    assert!(marker.exists(), "the child finished once frames flowed");
}

#[test]
fn a_program_that_cannot_start_is_recorded_and_exits_127() {
    let cgroup = applied_cgroup();
    let ran = run_in(
        &spec(&["/nonexistent/substrate-program"], &[], 4096),
        cgroup.path(),
    );
    assert_eq!(ran.code, 127);
    assert_eq!(
        ran.controls().last(),
        Some(&&Control::SpawnFailed {
            errno: libc::ENOENT
        })
    );
}

#[test]
fn the_run_spec_round_trips_through_parts_without_a_dollar() {
    let large = "$(X)".repeat(60_000);
    let spec = spec(
        &["/usr/bin/printf", "%s", large.as_str()],
        &[("VALUE", "$(HOME)")],
        1024,
    );
    let parts = spec.encode_parts();
    assert!(parts.len() > 1, "a large spec is split");
    for part in &parts {
        assert!(part.len() <= MAX_PART_BYTES);
        assert!(!part.contains('$'));
        assert!(
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
    }
    assert_eq!(RunSpec::decode_parts(&parts).expect("decode"), spec);

    for bad in [
        vec![],
        vec!["not base64 $".to_owned()],
        vec![base64url(br#"{"argv":[],"env":{},"memory_bytes":1,"cpu_millicores":1,"output_bytes":1}"#)],
        vec![base64url(br#"{"argv":["a"],"env":{},"memory_bytes":1,"cpu_millicores":1,"output_bytes":1,"image":"x"}"#)],
        vec![base64url(br#"{"argv":["a"],"env":{"A=B":"c"},"memory_bytes":1,"cpu_millicores":1,"output_bytes":1}"#)],
        vec![base64url(b"{\"argv\":[\"a\\u0000b\"],\"env\":{},\"memory_bytes\":1,\"cpu_millicores\":1,\"output_bytes\":1}")],
        vec!["A".repeat(MAX_PART_BYTES + 1)],
    ] {
        assert!(RunSpec::decode_parts(&bad).is_err(), "{bad:?}");
    }
}

fn base64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
