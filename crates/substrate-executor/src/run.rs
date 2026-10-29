//! The `run` role: an exec Pod's PID 1 (design 34 § 7.3).
//!
//! 1. Read `memory.max`, `cpu.max` and `pids.max` back from the cgroup and write the `applied`
//!    record. A memory or CPU bound that differs from the spec refuses `limits-unapplied` and exits
//!    [`LIMITS_UNAPPLIED_EXIT`] without starting the argv; `pids.max` is reported, not compared
//!    (design 34 § 14 Q2).
//! 2. Spawn the argv without a shell, in a new process group, in exactly the spec's environment,
//!    with stdin `/dev/null` and stdout and stderr as two pipes.
//! 3. Drain both pipes continuously, framing each chunk with its stream. Past `output_bytes` on a
//!    stream, write `truncated` once and discard the rest of that stream.
//! 4. `TERM` to the executor is forwarded to the child's process group ([`forward_term`]).
//! 5. When the direct child ends, write its `exit` record and return its code, or 128 plus its
//!    signal. As PID 1, the executor's exit tears down the PID namespace.

use std::io::{self, Read as _, Write};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::frame::{Applied, Control, ExitRecord, Frame, MAX_CHUNK_BYTES, Refusal, Stream};
use crate::spec::RunSpec;

/// The exit status of a run whose bounds were not applied; the argv did not start.
pub const LIMITS_UNAPPLIED_EXIT: i32 = 125;

/// The exit status of a run whose argv could not be started.
pub const SPAWN_FAILED_EXIT: i32 = 127;

/// The CFS period kubelet writes into `cpu.max` beside the quota is what the quota is read
/// against; the cgroup states both, so nothing here assumes kubelet's 100 ms.
const MILLICORES_PER_CORE: u64 = 1000;

/// The most bytes read from one cgroup interface file.
const CGROUP_FILE_BYTES: u64 = 256;

/// How long output is still drained after the direct child ends, for a descendant that holds a
/// pipe open. Output the child itself wrote before ending is already in the pipe.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);

/// The child's process group, once it runs; `TERM` is forwarded to it.
static CHILD_GROUP: AtomicI32 = AtomicI32::new(0);

/// Installs the `TERM` handler that forwards the signal to the child's process group. The binary
/// installs it once, before [`run`]; a library caller need not.
pub fn forward_term() {
    extern "C" fn forward(_: libc::c_int) {
        let group = CHILD_GROUP.load(Ordering::SeqCst);
        if group > 0 {
            // SAFETY: kill is async-signal-safe; a negative pid names the child's process group.
            unsafe {
                libc::kill(-group, libc::SIGTERM);
            }
        }
    }
    // SAFETY: the handler only loads an atomic and calls kill, both async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            forward as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
}

/// Reads the bounds the cgroup in `cgroup_root` applies.
pub fn read_applied(cgroup_root: &Path) -> Applied {
    let memory_bytes =
        read_cgroup_file(&cgroup_root.join("memory.max")).and_then(|text| text.parse::<u64>().ok());
    let cpu_millicores =
        read_cgroup_file(&cgroup_root.join("cpu.max")).and_then(|text| millicores(&text));
    let pids_max = read_cgroup_file(&cgroup_root.join("pids.max"));
    Applied {
        memory_bytes,
        cpu_millicores,
        pids_max,
    }
}

/// `<quota> <period>` as whole millicores, or `None` for `max` or a quota that is no whole number
/// of them.
fn millicores(text: &str) -> Option<u64> {
    let (quota, period) = text.split_once(' ')?;
    let quota: u64 = quota.parse().ok()?;
    let period: u64 = period.parse().ok()?;
    if period == 0 {
        return None;
    }
    let scaled = quota.checked_mul(MILLICORES_PER_CORE)?;
    (scaled % period == 0).then_some(scaled / period)
}

fn read_cgroup_file(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(CGROUP_FILE_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn write_frame(out: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let line = frame.encode().map_err(io::Error::other)?;
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

enum Event {
    Chunk(Stream, Vec<u8>),
    Closed,
    Exited(io::Result<ExitStatus>),
}

/// Runs `spec` under the cgroup whose interface files are in `cgroup_root`, writing every frame to
/// `out`, and returns the status the executor exits with.
///
/// # Errors
///
/// Only a failure to write a frame to `out`.
///
/// # Panics
///
/// Never: the child's stdout and stderr are requested as pipes, so the spawned child has both.
pub fn run(spec: &RunSpec, cgroup_root: &Path, out: &mut impl Write) -> io::Result<i32> {
    let applied = read_applied(cgroup_root);
    let unapplied = applied.memory_bytes != Some(spec.memory_bytes)
        || applied.cpu_millicores != Some(spec.cpu_millicores);
    write_frame(out, &Frame::Control(Control::Applied(applied)))?;
    if unapplied {
        write_frame(
            out,
            &Frame::Control(Control::Refused(Refusal::LimitsUnapplied)),
        )?;
        return Ok(LIMITS_UNAPPLIED_EXIT);
    }

    let Some((program, arguments)) = spec.argv.split_first() else {
        write_frame(
            out,
            &Frame::Control(Control::SpawnFailed {
                errno: libc::EINVAL,
            }),
        )?;
        return Ok(SPAWN_FAILED_EXIT);
    };
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let errno = error.raw_os_error().unwrap_or(0);
            write_frame(out, &Frame::Control(Control::SpawnFailed { errno }))?;
            return Ok(SPAWN_FAILED_EXIT);
        }
    };
    if let Ok(group) = i32::try_from(child.id()) {
        CHILD_GROUP.store(group, Ordering::SeqCst);
    }

    let (events, received) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    spawn_reader(Stream::Stdout, stdout, events.clone());
    spawn_reader(Stream::Stderr, stderr, events.clone());
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = events.send(Event::Exited(status));
    });

    let mut output = Output::new(spec.output_bytes);
    let mut open_streams = 2;
    let mut exited: Option<io::Result<ExitStatus>> = None;
    let mut deadline: Option<Instant> = None;
    while exited.is_none() || open_streams > 0 {
        let event = match deadline {
            None => match received.recv() {
                Ok(event) => event,
                Err(_) => break,
            },
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                match received.recv_timeout(left) {
                    Ok(event) => event,
                    Err(_) => break,
                }
            }
        };
        match event {
            Event::Chunk(stream, bytes) => output.write(out, stream, &bytes)?,
            Event::Closed => open_streams -= 1,
            Event::Exited(status) => {
                exited = Some(status);
                deadline = Some(Instant::now() + DRAIN_AFTER_EXIT);
            }
        }
    }
    CHILD_GROUP.store(0, Ordering::SeqCst);

    let (record, code) = match exited {
        Some(Ok(status)) => exit_record(status),
        _ => (ExitRecord::Signal("unknown".to_owned()), 128),
    };
    write_frame(out, &Frame::Control(Control::Exit(record)))?;
    Ok(code)
}

fn spawn_reader(
    stream: Stream,
    mut pipe: impl io::Read + Send + 'static,
    events: mpsc::Sender<Event>,
) {
    std::thread::spawn(move || {
        let mut buffer = vec![0_u8; MAX_CHUNK_BYTES];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if events
                        .send(Event::Chunk(stream, buffer[..read].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = events.send(Event::Closed);
    });
}

/// Per-stream output accounting against the spec's bound.
struct Output {
    limit: u64,
    written: [u64; 2],
    truncated: [bool; 2],
}

impl Output {
    const fn new(limit: u64) -> Self {
        Self {
            limit,
            written: [0; 2],
            truncated: [false; 2],
        }
    }

    fn write(&mut self, out: &mut impl Write, stream: Stream, bytes: &[u8]) -> io::Result<()> {
        let index = match stream {
            Stream::Stdout => 0,
            Stream::Stderr => 1,
        };
        if self.truncated[index] {
            return Ok(());
        }
        let room = self.limit.saturating_sub(self.written[index]);
        let kept = usize::try_from(room).unwrap_or(usize::MAX).min(bytes.len());
        if kept > 0 {
            write_frame(
                out,
                &Frame::Output {
                    stream,
                    bytes: bytes[..kept].to_vec(),
                },
            )?;
            self.written[index] += u64::try_from(kept).expect("usize fits u64");
        }
        if kept < bytes.len() {
            self.truncated[index] = true;
            write_frame(out, &Frame::Control(Control::Truncated(stream)))?;
        }
        Ok(())
    }
}

fn exit_record(status: ExitStatus) -> (ExitRecord, i32) {
    if let Some(code) = status.code() {
        return (ExitRecord::Code(code), code);
    }
    let signal = status.signal().unwrap_or(0);
    (ExitRecord::Signal(signal_name(signal)), 128 + signal)
}

/// The lower-case name of a signal, or `sig<n>` for one without a standard name.
fn signal_name(signal: i32) -> String {
    let name = match signal {
        libc::SIGHUP => "hup",
        libc::SIGINT => "int",
        libc::SIGQUIT => "quit",
        libc::SIGILL => "ill",
        libc::SIGTRAP => "trap",
        libc::SIGABRT => "abrt",
        libc::SIGBUS => "bus",
        libc::SIGFPE => "fpe",
        libc::SIGKILL => "kill",
        libc::SIGUSR1 => "usr1",
        libc::SIGSEGV => "segv",
        libc::SIGUSR2 => "usr2",
        libc::SIGPIPE => "pipe",
        libc::SIGALRM => "alrm",
        libc::SIGTERM => "term",
        libc::SIGXCPU => "xcpu",
        libc::SIGXFSZ => "xfsz",
        libc::SIGSYS => "sys",
        _ => return format!("sig{signal}"),
    };
    name.to_owned()
}
