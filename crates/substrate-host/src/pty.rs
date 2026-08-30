//! Pseudoterminal allocation for `pty` sessions (design 13).
//!
//! Three things live here and nothing else: the pair, the async master, and the startup probe that
//! decides whether `sessions.pty` may be published at all.
//!
//! **The master never crosses the confinement boundary.** The child inherits the slave as its three
//! standard descriptors and nothing more; the master is marked close-on-exec here so no fork of
//! this process can carry it into a sandbox. That is what makes a second, in-band signal path — the
//! line discipline turning the interrupt character into a signal for the foreground process group —
//! safe: the input queue a child can push characters into is its own.
//!
//! **The controlling terminal is taken inside the sandbox, after bubblewrap's `setsid`.** The
//! shared confinement path passes `--new-session`, which `bwrap(1)` documents as calling `setsid()`
//! and disconnecting the sandbox from the controlling terminal — the mitigation it names for
//! CVE-2017-5226. Inheriting the slave as descriptors 0, 1 and 2 is therefore not enough: without a
//! controlling terminal there is no foreground process group, so no `SIGWINCH` on a resize and no
//! hangup when the master closes. Dropping `--new-session` to get the same effect would weaken the
//! confinement floor of every non-pty exec to serve one feature, which is the silent degradation
//! invariant 3 forbids (design 13). The interposition below runs *after* that `setsid`, and the
//! probe proves it did rather than assuming it.

use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use substrate_wire::PtyWindow;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, ReadBuf};

/// The interposition that acquires the controlling terminal, after bubblewrap's own `setsid`.
///
/// `setsid(1)` from util-linux calls `setsid()` and then `ioctl(0, TIOCSCTTY, …)`, which is exactly
/// the pair of calls the kernel requires and which no shell can make. It does not fork here: it
/// forks only when it is already a process-group leader, and the process bubblewrap execs is a
/// member of pid 1's group rather than the leader of its own. Its absence is not routed around —
/// the probe below fails, `sessions.pty` stays absent, and every terminal request is refused by
/// name (invariant 3).
pub(crate) const CONTROLLING_TERMINAL_ARGV: [&str; 2] = ["/usr/bin/setsid", "--ctty"];

/// How long the startup probe waits for the confined child at each step.
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// The throwaway sandbox the probe proves the mechanism in — the same confinement floor an
/// admitted session gets, minus the cgroup and the workspace this probe has no use for.
const SANDBOX_ARGV: [&str; 27] = [
    "--unshare-user",
    "--unshare-ipc",
    "--unshare-pid",
    "--unshare-net",
    "--unshare-uts",
    "--new-session",
    "--die-with-parent",
    "--clearenv",
    "--ro-bind",
    "/usr",
    "/usr",
    "--ro-bind-try",
    "/bin",
    "/bin",
    "--ro-bind-try",
    "/lib",
    "/lib",
    "--ro-bind-try",
    "/lib64",
    "/lib64",
    "--proc",
    "/proc",
    "--dev",
    "/dev",
    "--tmpfs",
    "/tmp",
    "--",
];

/// One allocated pseudoterminal. The parent keeps `master`; the child inherits `slave`.
pub(crate) struct PtyPair {
    pub(crate) master: OwnedFd,
    pub(crate) slave: OwnedFd,
}

/// Allocates a pair with the declared window already set, so the child's first `TIOCGWINSZ`
/// answers with what the client asked for rather than with a kernel default.
pub(crate) fn open(window: PtyWindow) -> io::Result<PtyPair> {
    let size = nix::pty::Winsize {
        ws_row: window.rows,
        ws_col: window.columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let opened = nix::pty::openpty(Some(&size), None).map_err(io::Error::from)?;
    // `openpty(3)` leaves both ends inheritable. The master must never reach a sandbox: a child
    // holding it would keep the terminal alive after this process let go, so no close of ours
    // could hang it up.
    set_close_on_exec(opened.master.as_raw_fd())?;
    Ok(PtyPair {
        master: opened.master,
        slave: opened.slave,
    })
}

fn set_close_on_exec(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor this process owns for the whole call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above; `F_SETFD` takes an int and touches no memory of ours.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor this process owns for the whole call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Sets the window on the master. The kernel signals the foreground process group of the terminal,
/// which is what makes the child observe the change instead of being told about it.
pub(crate) fn set_window(fd: RawFd, window: PtyWindow) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: window.rows,
        ws_col: window.columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `fd` is an open terminal descriptor this process owns, and `size` is a live
    // `winsize` for the whole call — exactly what `TIOCSWINSZ` reads.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, std::ptr::from_ref(&size)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_window(fd: RawFd) -> io::Result<PtyWindow> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: as in `set_window`; `TIOCGWINSZ` writes one `winsize` into memory we own.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, std::ptr::from_mut(&mut size)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PtyWindow {
        columns: size.ws_col,
        rows: size.ws_row,
    })
}

/// The master end, readable and writable from the async runtime and resizable from anywhere.
pub(crate) struct PtyMaster {
    fd: AsyncFd<OwnedFd>,
}

impl PtyMaster {
    pub(crate) fn new(master: OwnedFd) -> io::Result<Self> {
        set_nonblocking(master.as_raw_fd())?;
        Ok(Self {
            fd: AsyncFd::new(master)?,
        })
    }

    pub(crate) async fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let mut written = 0;
        while written < bytes.len() {
            let mut guard = self.fd.writable().await?;
            let outcome = guard.try_io(|inner| {
                let remaining = &bytes[written..];
                // SAFETY: `remaining` is a live slice for the call and the descriptor is owned by
                // `inner` for at least as long as this closure runs.
                let count = unsafe {
                    libc::write(
                        inner.get_ref().as_raw_fd(),
                        remaining.as_ptr().cast(),
                        remaining.len(),
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(usize::try_from(count).unwrap_or(0))
                }
            });
            match outcome {
                Ok(Ok(count)) => written = written.saturating_add(count),
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => {}
            }
        }
        Ok(())
    }

    pub(crate) fn resize(&self, window: PtyWindow) -> io::Result<()> {
        set_window(self.fd.get_ref().as_raw_fd(), window)
    }
}

/// The read half, so the shared bounded drain reads a terminal exactly as it reads a pipe.
pub(crate) struct PtyOutput(pub(crate) Arc<PtyMaster>);

impl AsyncRead for PtyOutput {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = match self.0.fd.poll_read_ready(context) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            };
            let unfilled = buffer.initialize_unfilled();
            let outcome = guard.try_io(|inner| {
                // SAFETY: `unfilled` is a live, initialised slice for the call, and the descriptor
                // outlives the closure.
                let count = unsafe {
                    libc::read(
                        inner.get_ref().as_raw_fd(),
                        unfilled.as_mut_ptr().cast(),
                        unfilled.len(),
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(usize::try_from(count).unwrap_or(0))
                }
            });
            match outcome {
                Ok(Ok(count)) => {
                    buffer.advance(count);
                    return Poll::Ready(Ok(()));
                }
                // A master whose every slave has closed reads `EIO`. That is this device's end of
                // file; reporting it as an error would make an ordinary exit look like a drain that
                // failed, and a failed drain is recorded as truncation.
                Ok(Err(error)) if error.raw_os_error() == Some(libc::EIO) => {
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_would_block) => {}
            }
        }
    }
}

/// What one confined child reported about the terminal it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SandboxedTerminal {
    /// The window the child read back with `TIOCGWINSZ` before any resize.
    pub(crate) initial: PtyWindow,
    /// The window it read back after the master was resized underneath it.
    pub(crate) resized: PtyWindow,
    /// The child's controlling terminal, as the kernel reports it — zero means it has none.
    pub(crate) controlling_terminal: u64,
}

/// Whether this host can give a confined process a working controlling terminal.
///
/// The whole mechanism, in a throwaway sandbox, and never a constant: a pair is allocated, made
/// controlling inside the sandbox after bubblewrap's `setsid`, and a size is round-tripped through
/// the child both before and after a resize. `sessions.pty` is published only when every clause
/// holds, so a host that cannot do this refuses every terminal request by name rather than serving
/// something quieter (invariant 3, invariant 4).
pub(crate) fn mechanism_is_provable(bubblewrap: &Path) -> bool {
    let initial = PtyWindow {
        columns: 97,
        rows: 37,
    };
    let resized = PtyWindow {
        columns: 132,
        rows: 43,
    };
    observe_sandboxed_terminal(bubblewrap, initial, resized).is_some_and(|observed| {
        observed.initial == initial
            && observed.resized == resized
            && observed.controlling_terminal != 0
    })
}

/// Runs one throwaway sandbox on a fresh pair and reports what the child observed.
///
/// The child is a shell reading `TIOCGWINSZ` — `stty size` is that ioctl — because that is the call
/// the acceptance names. Nothing is read from the environment: `COLUMNS` and `LINES` go stale at the
/// first resize, and this sandbox is `--clearenv` anyway.
pub(crate) fn observe_sandboxed_terminal(
    bubblewrap: &Path,
    initial: PtyWindow,
    resized: PtyWindow,
) -> Option<SandboxedTerminal> {
    if !bubblewrap.is_file() {
        return None;
    }
    let pair = open(initial).ok()?;
    let mut command = Command::new(bubblewrap);
    command
        .env_clear()
        .stdin(Stdio::from(pair.slave.try_clone().ok()?))
        .stdout(Stdio::from(pair.slave.try_clone().ok()?))
        .stderr(Stdio::from(pair.slave.try_clone().ok()?))
        .args(SANDBOX_ARGV)
        .args(CONTROLLING_TERMINAL_ARGV)
        .args([
            "/bin/sh",
            "-c",
            "printf 'A:%s %s\\n' \"$(stty size)\" \"$(cut -d' ' -f7 /proc/self/stat)\"; \
             read _ignored; printf 'B:%s\\n' \"$(stty size)\"",
        ]);
    // SAFETY: the closure runs after fork and calls only async-signal-safe entry points on a plain
    // descriptor number the parent holds open across the fork.
    let master_number = pair.master.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            if libc::close(master_number) != 0
                && io::Error::last_os_error().raw_os_error() != Some(libc::EBADF)
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().ok()?;
    // The parent's own copy goes now: while it is open the master never sees the child's exit.
    drop(pair.slave);
    let master = pair.master;
    let observed = observe_through(master.as_raw_fd(), resized);
    let _killed = child.kill();
    let _reaped = child.wait();
    drop(master);
    observed
}

/// The parent half of the probe conversation, with one deadline per step.
fn observe_through(master: RawFd, resized: PtyWindow) -> Option<SandboxedTerminal> {
    let mut transcript = Vec::new();
    let first = read_line_with_prefix(master, b"A:", &mut transcript)?;
    let mut fields = first.split_whitespace();
    let rows: u16 = fields.next()?.parse().ok()?;
    let columns: u16 = fields.next()?.parse().ok()?;
    let controlling_terminal: u64 = fields.next()?.parse().ok()?;
    set_window(master, resized).ok()?;
    // Read back through the same ioctl the child will use, so a kernel that took the size and a
    // parent that only asked for it are told apart here rather than by the child.
    if read_window(master).ok()? != resized {
        return None;
    }
    write_all_blocking(master, b"\n")?;
    let second = read_line_with_prefix(master, b"B:", &mut transcript)?;
    let mut fields = second.split_whitespace();
    let resized_rows: u16 = fields.next()?.parse().ok()?;
    let resized_columns: u16 = fields.next()?.parse().ok()?;
    Some(SandboxedTerminal {
        initial: PtyWindow { columns, rows },
        resized: PtyWindow {
            columns: resized_columns,
            rows: resized_rows,
        },
        controlling_terminal,
    })
}

fn read_line_with_prefix(master: RawFd, prefix: &[u8], transcript: &mut Vec<u8>) -> Option<String> {
    let deadline = Instant::now().checked_add(PROBE_DEADLINE)?;
    loop {
        if let Some(line) = take_line(transcript, prefix) {
            return Some(line);
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        if !wait_readable(master, remaining) {
            return None;
        }
        let mut buffer = [0_u8; 1024];
        // SAFETY: `buffer` is live for the call and owned by this frame.
        let count = unsafe {
            libc::read(
                master,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                buffer.len(),
            )
        };
        if count <= 0 {
            return None;
        }
        transcript.extend_from_slice(&buffer[..usize::try_from(count).ok()?]);
        if transcript.len() > 64 * 1024 {
            return None;
        }
    }
}

/// The first complete line carrying `prefix`, with the prefix removed.
///
/// The transcript carries the line discipline's echo of what the parent typed as well as what the
/// child printed, so a line is selected by its prefix and never by its position.
fn take_line(transcript: &[u8], prefix: &[u8]) -> Option<String> {
    let mut start = 0;
    while start < transcript.len() {
        let end = transcript[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| start + offset)?;
        let line = &transcript[start..end];
        if let Some(rest) = line.strip_prefix(prefix) {
            return String::from_utf8(rest.to_vec()).ok();
        }
        start = end + 1;
    }
    None
}

fn wait_readable(fd: RawFd, within: Duration) -> bool {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let milliseconds = i32::try_from(within.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one live `pollfd` this frame owns, and a count that matches it.
    let ready = unsafe { libc::poll(std::ptr::from_mut(&mut descriptor), 1, milliseconds) };
    ready > 0
}

fn write_all_blocking(fd: RawFd, bytes: &[u8]) -> Option<()> {
    let mut written = 0;
    while written < bytes.len() {
        let remaining = &bytes[written..];
        // SAFETY: `remaining` is live for the call.
        let count = unsafe {
            libc::write(
                fd,
                remaining.as_ptr().cast::<libc::c_void>(),
                remaining.len(),
            )
        };
        if count <= 0 {
            return None;
        }
        written = written.saturating_add(usize::try_from(count).ok()?);
    }
    Some(())
}

#[cfg(test)]
pub(crate) fn observe_sandboxed_window(
    bubblewrap: &Path,
    initial: PtyWindow,
    resized: PtyWindow,
) -> Option<(PtyWindow, PtyWindow)> {
    observe_sandboxed_terminal(bubblewrap, initial, resized)
        .map(|observed| (observed.initial, observed.resized))
}

/// Runs a confined child on a fresh terminal, closes the master, and reports how it ended.
///
/// The child leaves `SIGHUP` at its default disposition, so a hangup is the only way it can stop
/// before its own deadline. Bubblewrap reports a signalled child as `128 + signal`.
#[cfg(test)]
pub(crate) fn observe_sandboxed_hangup(bubblewrap: &Path) -> Option<i32> {
    if !bubblewrap.is_file() {
        return None;
    }
    let window = PtyWindow {
        columns: 80,
        rows: 24,
    };
    let pair = open(window).ok()?;
    let mut command = Command::new(bubblewrap);
    command
        .env_clear()
        .stdin(Stdio::from(pair.slave.try_clone().ok()?))
        .stdout(Stdio::from(pair.slave.try_clone().ok()?))
        .stderr(Stdio::from(pair.slave.try_clone().ok()?))
        .args(SANDBOX_ARGV)
        .args(CONTROLLING_TERMINAL_ARGV)
        .args(["/bin/sh", "-c", "printf 'READY\n'; exec /usr/bin/sleep 300"]);
    let master_number = pair.master.as_raw_fd();
    // SAFETY: the closure runs after fork and closes one plain descriptor number.
    unsafe {
        command.pre_exec(move || {
            let _closed = libc::close(master_number);
            Ok(())
        });
    }
    let mut child = command.spawn().ok()?;
    drop(pair.slave);
    let master = pair.master;
    let mut transcript = Vec::new();
    read_line_with_prefix(master.as_raw_fd(), b"READY", &mut transcript)?;
    // The hangup itself: the parent lets go of the only remaining master.
    drop(master);
    let status = child.wait().ok()?;
    status.code()
}

#[cfg(test)]
pub(crate) fn observe_sandboxed_controlling_terminal(bubblewrap: &Path) -> Option<u64> {
    let window = PtyWindow {
        columns: 80,
        rows: 24,
    };
    observe_sandboxed_terminal(bubblewrap, window, window)
        .map(|observed| observed.controlling_terminal)
}

/// Reconstructs an owned descriptor from a raw one the caller has just given up.
#[allow(dead_code)]
pub(crate) unsafe fn owned(fd: RawFd) -> OwnedFd {
    // SAFETY: the caller states this descriptor is open and no longer owned anywhere else.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

#[cfg(test)]
mod tests {
    use substrate_wire::PtyWindow;

    /// The acceptance's middle clause, at the layer that owns it: a window declared on the master
    /// is the window the confined child reads back with `TIOCGWINSZ`, and a resize applied while
    /// the child runs is observed by the same call. `stty size` is `TIOCGWINSZ`; nothing here
    /// reads an environment variable, because `COLUMNS` goes stale at the first resize and this
    /// sandbox has `--clearenv` anyway.
    ///
    /// Needs `bwrap` and nothing else — no cgroup delegation — because the mechanism the
    /// capability fact publishes needs none. Where `bwrap` cannot run the mechanism, the fact is
    /// **absent**, and this asserts that instead (invariant 3).
    #[test]
    fn pty_resize_is_applied_and_observed() {
        let bubblewrap = std::path::PathBuf::from("/usr/bin/bwrap");
        let initial = PtyWindow {
            columns: 97,
            rows: 37,
        };
        let resized = PtyWindow {
            columns: 132,
            rows: 43,
        };
        let observed = crate::pty::observe_sandboxed_window(&bubblewrap, initial, resized);
        if !bubblewrap.is_file() {
            assert_eq!(
                observed, None,
                "without the confinement backend the mechanism is unprovable and the fact absent"
            );
            assert!(!crate::pty::mechanism_is_provable(&bubblewrap));
            return;
        }
        let observed = observed.expect("the confined child reported both windows");
        assert_eq!(
            observed,
            (initial, resized),
            "the child read back the declared window and then the applied resize"
        );
        assert!(crate::pty::mechanism_is_provable(&bubblewrap));
    }

    /// The acceptance's last clause: a confined child exits when the terminal hangs up.
    ///
    /// `sleep(1)` leaves `SIGHUP` at its default disposition, so it cannot exit for any other
    /// reason; bubblewrap reports a signalled child as `128 + signal`, which makes 129 the exact
    /// claim "the kernel hung up this terminal's foreground process group". That path exists only
    /// because the controlling terminal was acquired inside the sandbox: without one there is no
    /// foreground process group to signal, and the child would sit out its whole timeout.
    #[test]
    fn a_confined_terminal_hangs_up_when_the_master_closes() {
        let bubblewrap = std::path::PathBuf::from("/usr/bin/bwrap");
        if !bubblewrap.is_file() {
            return;
        }
        assert_eq!(
            crate::pty::observe_sandboxed_hangup(&bubblewrap),
            Some(128 + libc::SIGHUP),
            "closing the master must hang the child up, not leave it running"
        );
    }

    /// Design 13: the controlling terminal is taken **inside** the sandbox, after bubblewrap's
    /// `setsid`, and `--new-session` is never dropped to get the same effect. Without a
    /// controlling terminal there is no foreground process group, so no `SIGWINCH` and no hangup.
    #[test]
    fn the_controlling_terminal_is_taken_inside_the_sandbox() {
        let bubblewrap = std::path::PathBuf::from("/usr/bin/bwrap");
        if !bubblewrap.is_file() {
            return;
        }
        let tty_number = crate::pty::observe_sandboxed_controlling_terminal(&bubblewrap)
            .expect("the confined child reported its controlling terminal");
        assert_ne!(
            tty_number, 0,
            "a child with no controlling terminal reports tty_nr 0 and gets neither SIGWINCH nor \
             a hangup"
        );
    }
}
