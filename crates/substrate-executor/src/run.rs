//! The `run` role: an exec Pod's PID 1 (design 34 § 7.3).
//!
//! 1. Read `memory.max`, `cpu.max` and `pids.max` back from the cgroup and write the `applied`
//!    record. A memory or CPU bound that differs from the spec refuses `limits-unapplied` and exits
//!    [`LIMITS_UNAPPLIED_EXIT`] without starting the argv; `pids.max` is reported, not compared
//!    (design 34 § 14 Q2).
//! 2. Make the executor non-dumpable, then spawn the argv without a shell, in a new process group,
//!    in exactly the spec's environment, with stdin `/dev/null` and stdout and stderr as two
//!    pipes. The child runs as the executor's uid; non-dumpable, `/proc/<executor>/fd` is closed to
//!    it, so it cannot reopen the executor's own stdout and append a frame.
//! 3. Drain both pipes through a bounded queue, so a stdout nobody reads holds the child on its
//!    pipes rather than queueing its output in memory. Each stream is framed in full
//!    [`MAX_CHUNK_BYTES`] chunks and its remainder once, when it closes or the run ends, so a
//!    stream of L bytes takes at most `ceil(L / MAX_CHUNK_BYTES)` lines, as the log budget of
//!    `exec.output-limit-bytes` counts. Past `output_bytes` on a stream, write `truncated` once and
//!    discard the rest of that stream.
//! 4. `TERM` to the executor is forwarded to the child's process group ([`forward_term`]). A `TERM`
//!    that arrives before the group is stored is recorded and forwarded once it is, not dropped;
//!    the binary blocks `TERM` until the handler is installed, so none is lost before that either.
//!    The child never inherits a blocked `TERM`.
//! 5. When the direct child ends, write its `exit` record and return its code, or 128 plus its
//!    signal. [`run_as_init`] also reaps every orphan reparented to it, as PID 1 must. As PID 1,
//!    the executor's exit tears down the PID namespace.

use std::io::{self, Read as _, Write};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
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

/// The most pipe reads queued between the readers and the frame writer. Past it a reader stops
/// reading, the pipe fills and the child blocks on its next write.
const QUEUED_READS: usize = 8;

/// The child's process group, once it runs; `TERM` is forwarded to it.
static CHILD_GROUP: AtomicI32 = AtomicI32::new(0);

/// A `TERM` reached the handler before the child's group was stored.
static TERM_PENDING: AtomicBool = AtomicBool::new(false);

/// Blocks `TERM` on the calling thread. The binary calls it first, so a `TERM` that arrives
/// before [`forward_term`] installs the handler is queued, even to a PID 1 without a handler,
/// rather than dropped; [`forward_term`] unblocks it.
pub fn block_term() {
    set_term_blocked(true);
}

fn set_term_blocked(blocked: bool) {
    // SAFETY: the set is initialized by sigemptyset before use, and pthread_sigmask changes only
    // the calling thread's mask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        let how = if blocked {
            libc::SIG_BLOCK
        } else {
            libc::SIG_UNBLOCK
        };
        libc::pthread_sigmask(how, &raw const set, std::ptr::null_mut());
    }
}

/// Makes this process non-dumpable and reads that back: `/proc/<pid>/fd` and `ptrace` are then
/// closed to a same-uid process without `CAP_SYS_PTRACE`, such as the child. A child's `execve`
/// makes the child dumpable again; this flag stays the executor's.
fn make_non_dumpable() -> Result<(), i32> {
    // SAFETY: both prctl options take integer arguments only and touch no memory of ours.
    let (set, dumpable) = unsafe {
        (
            libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0),
            libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0),
        )
    };
    if set != 0 {
        return Err(io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EPERM));
    }
    if dumpable != 0 {
        return Err(libc::EPERM);
    }
    Ok(())
}

/// Installs the `TERM` handler that forwards the signal to the child's process group, then
/// unblocks `TERM` on the calling thread. A `TERM` that arrives before the group is known, or that
/// was pending while [`block_term`] held it, is recorded and forwarded by [`run`] as soon as the
/// group is stored. The binary installs it once, before [`run`]; a library caller need not.
pub fn forward_term() {
    extern "C" fn forward(_: libc::c_int) {
        let group = CHILD_GROUP.load(Ordering::SeqCst);
        if group > 0 {
            // SAFETY: kill is async-signal-safe; a negative pid names the child's process group.
            unsafe {
                libc::kill(-group, libc::SIGTERM);
            }
        } else {
            TERM_PENDING.store(true, Ordering::SeqCst);
        }
    }
    // SAFETY: the handler only touches atomics and calls kill, both async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            forward as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
    set_term_blocked(false);
}

/// Stores the child's group, then forwards a `TERM` that arrived before it was known. A `TERM`
/// after the store is the handler's own to forward, so each is forwarded at least once.
fn store_child_group(group: i32) {
    CHILD_GROUP.store(group, Ordering::SeqCst);
    if TERM_PENDING.swap(false, Ordering::SeqCst) {
        // SAFETY: a negative pid names the child's process group.
        unsafe {
            libc::kill(-group, libc::SIGTERM);
        }
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
    Closed(Stream),
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
    run_with(spec, cgroup_root, out, Reaping::DirectChild)
}

/// [`run`] as an exec Pod's PID 1: it also becomes a child subreaper and reaps every process
/// reparented to it, so an exited orphan does not stay a zombie holding a `pids.max` slot. It
/// waits on every child of this process, so a caller that has children of its own uses [`run`].
///
/// # Errors
///
/// Only a failure to write a frame to `out`.
pub fn run_as_init(spec: &RunSpec, cgroup_root: &Path, out: &mut impl Write) -> io::Result<i32> {
    run_with(spec, cgroup_root, out, Reaping::EveryChild)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reaping {
    DirectChild,
    EveryChild,
}

fn run_with(
    spec: &RunSpec,
    cgroup_root: &Path,
    out: &mut impl Write,
    reaping: Reaping,
) -> io::Result<i32> {
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

    let mut child = match spawn(spec, reaping) {
        Ok(child) => child,
        Err(errno) => {
            write_frame(out, &Frame::Control(Control::SpawnFailed { errno }))?;
            return Ok(SPAWN_FAILED_EXIT);
        }
    };

    let (events, received) = mpsc::sync_channel(QUEUED_READS);
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    spawn_reader(Stream::Stdout, stdout, events.clone());
    spawn_reader(Stream::Stderr, stderr, events.clone());
    match reaping {
        Reaping::DirectChild => {
            std::thread::spawn(move || {
                let status = child.wait();
                let _ = events.send(Event::Exited(status));
            });
        }
        Reaping::EveryChild => {
            let pid = libc::pid_t::try_from(child.id()).unwrap_or(0);
            drop(child);
            std::thread::spawn(move || reap_every_child(pid, &events));
        }
    }

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
            Event::Closed(stream) => {
                open_streams -= 1;
                output.flush(out, stream)?;
            }
            Event::Exited(status) => {
                exited = Some(status);
                deadline = Some(Instant::now() + DRAIN_AFTER_EXIT);
            }
        }
    }
    CHILD_GROUP.store(0, Ordering::SeqCst);
    output.flush(out, Stream::Stdout)?;
    output.flush(out, Stream::Stderr)?;

    let (record, code) = match exited {
        Some(Ok(status)) => exit_record(status),
        _ => (ExitRecord::Signal("unknown".to_owned()), 128),
    };
    write_frame(out, &Frame::Control(Control::Exit(record)))?;
    Ok(code)
}

/// Makes the executor non-dumpable, then spawns the argv and stores its process group, or returns
/// the errno the run's `spawn_failed` record carries.
fn spawn(spec: &RunSpec, reaping: Reaping) -> Result<Child, i32> {
    let (program, arguments) = spec.argv.split_first().ok_or(libc::EINVAL)?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    make_non_dumpable()?;
    if reaping == Reaping::EveryChild {
        // SAFETY: PR_SET_CHILD_SUBREAPER takes an integer argument only.
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        }
    }
    let child = command
        .spawn()
        .map_err(|error| error.raw_os_error().unwrap_or(0))?;
    if let Ok(group) = i32::try_from(child.id()) {
        store_child_group(group);
    }
    Ok(child)
}

/// Waits on every child of this process until none is left, sending the direct child's status
/// when `direct` ends. Orphans reparented here are reaped as they end.
fn reap_every_child(direct: libc::pid_t, events: &mpsc::SyncSender<Event>) {
    let mut reported = false;
    loop {
        let mut status = 0;
        // SAFETY: waitpid writes only the status integer it is handed.
        let reaped = unsafe { libc::waitpid(-1, &raw mut status, 0) };
        if reaped == direct && !reported {
            reported = true;
            let _ = events.send(Event::Exited(Ok(ExitStatus::from_raw(status))));
        } else if reaped < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if !reported {
                let _ = events.send(Event::Exited(Err(error)));
            }
            return;
        }
    }
}

fn spawn_reader(
    stream: Stream,
    mut pipe: impl io::Read + Send + 'static,
    events: mpsc::SyncSender<Event>,
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
        let _ = events.send(Event::Closed(stream));
    });
}

/// Per-stream output accounting against the spec's bound. A stream's kept bytes are framed only
/// as full [`MAX_CHUNK_BYTES`] chunks until [`Output::flush`] frames its remainder, so the number
/// of lines a stream takes depends on its length, never on how the child split its writes.
struct Output {
    limit: u64,
    written: [u64; 2],
    truncated: [bool; 2],
    pending: [Vec<u8>; 2],
}

const fn index(stream: Stream) -> usize {
    match stream {
        Stream::Stdout => 0,
        Stream::Stderr => 1,
    }
}

impl Output {
    const fn new(limit: u64) -> Self {
        Self {
            limit,
            written: [0; 2],
            truncated: [false; 2],
            pending: [Vec::new(), Vec::new()],
        }
    }

    fn write(&mut self, out: &mut impl Write, stream: Stream, bytes: &[u8]) -> io::Result<()> {
        let index = index(stream);
        if self.truncated[index] {
            return Ok(());
        }
        let room = self.limit.saturating_sub(self.written[index]);
        let kept = usize::try_from(room).unwrap_or(usize::MAX).min(bytes.len());
        self.pending[index].extend_from_slice(&bytes[..kept]);
        self.written[index] += u64::try_from(kept).expect("usize fits u64");
        while self.pending[index].len() >= MAX_CHUNK_BYTES {
            let chunk: Vec<u8> = self.pending[index].drain(..MAX_CHUNK_BYTES).collect();
            write_frame(
                out,
                &Frame::Output {
                    stream,
                    bytes: chunk,
                },
            )?;
        }
        if kept < bytes.len() {
            self.flush(out, stream)?;
            self.truncated[index] = true;
            write_frame(out, &Frame::Control(Control::Truncated(stream)))?;
        }
        Ok(())
    }

    /// Frames what is left of `stream`; called once it closes or the run ends.
    fn flush(&mut self, out: &mut impl Write, stream: Stream) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending[index(stream)]);
        if pending.is_empty() {
            return Ok(());
        }
        write_frame(
            out,
            &Frame::Output {
                stream,
                bytes: pending,
            },
        )
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
