//! A run's output delivered while it runs (design 32 § 3).
//!
//! A profile asks for it with [`OutputDelivery::Live`](super::OutputDelivery), which requires the
//! fact `output_live`, and the embedder reads it with
//! [`ConfinementAdapter::read_output`](super::ConfinementAdapter::read_output). Every adapter that
//! reads its own pipes (`local`, `systemd` around `local`, `macos` and `oci`) holds one
//! [`LiveOutput`] per run: the shared drain fills the record first and then queues the bytes it
//! kept, at most `queued_chunks` chunks waiting. `linux` reads the host driver's own queue and
//! shares only [`Progress`], so the events are numbered and closed out the same way on each.
//!
//! Backpressure is terminal (ADR 0031): a chunk that waits [`STALL_DEADLINE`] for a slot stops
//! delivery and ends the run as its `signal(Kill)` does. The drain keeps reading, so the child
//! never blocks on its pipes and the record stays the complete account.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::{ConfinementError, RunOutput};
use crate::{DriverError, PipeStream};

/// The most chunks a live run's queue holds: the host's `PIPE_QUEUED_FRAMES`.
pub const MAX_QUEUED_CHUNKS: u32 = 16;

/// `Live` together with a retention other than `Head` on either stream: the record could then
/// not begin with the delivered bytes (design 32 § 3). 422, class `Refused`.
pub const LIVE_OUTPUT_RETENTION_INVALID: &str = "confinement.live-output-retention-invalid";

/// `run_with_stdin` with a `Live` profile: it reads no events, so the run would stall. 422,
/// class `Refused`.
pub const LIVE_OUTPUT_UNREAD: &str = "confinement.live-output-unread";

/// How long a chunk waits for a queue slot before the run is ended: the host's
/// `PIPE_OUTPUT_STALL_TIMEOUT` (ADR 0031).
pub(crate) const STALL_DEADLINE: Duration = Duration::from_secs(1);

/// One event of a live run's output.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Bytes of one stream; `offset` is how many bytes of that stream came before them.
    Chunk {
        stream: PipeStream,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// The stream's delivered bytes reached its output bound; later bytes were drained and not
    /// delivered.
    Bounded { stream: PipeStream },
    /// No chunk was taken within the stall deadline: delivery stopped and the run was ended
    /// (`session.output-backpressure`).
    Stalled,
}

/// `session.not-pipe`: a live read of a run whose profile asked for the record only.
pub(crate) fn not_live() -> ConfinementError {
    DriverError::refused(
        substrate_wire::SESSION_NOT_PIPE,
        "The run's output is not delivered live.",
        "run",
    )
    .into()
}

/// The [`Kill`] of an adapter whose run is the process group its leader `pid` leads: `KILL` to
/// the group under `reaped`, the lock that keeps the leader unreaped, as its `signal(Kill)`
/// sends it.
pub(crate) fn group_kill(pid: i32, reaped: Arc<parking_lot::Mutex<bool>>) -> Kill {
    Box::new(move || {
        let reaped = reaped.lock();
        if !*reaped {
            // SAFETY: `kill` with a negative pid signals one process group; no memory is
            // involved. The leader is unreaped while the lock says so, so the id is this run's.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    })
}

/// `session.timeout-invalid`: a read that may not wait at all.
pub(crate) fn timeout_invalid() -> ConfinementError {
    DriverError::refused(
        substrate_wire::SESSION_TIMEOUT_INVALID,
        "A live output read timeout must be nonzero.",
        "timeout",
    )
    .into()
}

/// `session.read-timeout`: no event within the caller's deadline.
pub(crate) fn read_timeout() -> ConfinementError {
    DriverError::failed(
        substrate_wire::SESSION_READ_TIMEOUT,
        "No live output event arrived before the read deadline.",
    )
    .into()
}

const fn index(stream: PipeStream) -> usize {
    match stream {
        PipeStream::Stdout => 0,
        PipeStream::Stderr => 1,
    }
}

/// Where one reader is: each stream's delivered offset, and which of the events synthesized
/// after the queue it has returned.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    offsets: [u64; 2],
    /// Every chunk was read and the queue is over.
    exhausted: bool,
    /// The next synthesized event to consider: `Bounded` on stdout, on stderr, `Stalled`, then
    /// nothing.
    stage: u8,
}

impl Progress {
    /// Numbers the next chunk of `stream`.
    pub(crate) fn chunk(&mut self, stream: PipeStream, bytes: Vec<u8>) -> OutputEvent {
        let delivered = &mut self.offsets[index(stream)];
        let offset = *delivered;
        *delivered += bytes.len() as u64;
        OutputEvent::Chunk {
            stream,
            offset,
            bytes,
        }
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted
    }

    pub(crate) fn exhaust(&mut self) {
        self.exhausted = true;
    }

    /// The next event after the queue, in order and each at most once: `Bounded` for a stream
    /// whose record was truncated and whose every kept byte was delivered, `Stalled` when
    /// delivery stopped at the stall deadline, then `None`. `record` says, per stream, whether
    /// the record was truncated and how many bytes it holds.
    pub(crate) fn after_queue(
        &mut self,
        record: [(bool, u64); 2],
        stalled: bool,
    ) -> Option<OutputEvent> {
        while self.stage < 3 {
            let stage = self.stage;
            self.stage += 1;
            match stage {
                0 | 1 => {
                    let (truncated, length) = record[usize::from(stage)];
                    if truncated && self.offsets[usize::from(stage)] == length {
                        return Some(OutputEvent::Bounded {
                            stream: if stage == 0 {
                                PipeStream::Stdout
                            } else {
                                PipeStream::Stderr
                            },
                        });
                    }
                }
                _ => {
                    if stalled {
                        return Some(OutputEvent::Stalled);
                    }
                }
            }
        }
        None
    }
}

/// What `after_queue` reads of a record.
pub(crate) fn record_of(output: &RunOutput) -> [(bool, u64); 2] {
    [
        (output.stdout_truncated, output.stdout.len() as u64),
        (output.stderr_truncated, output.stderr.len() as u64),
    ]
}

struct Frame {
    stream: PipeStream,
    bytes: Vec<u8>,
}

struct Reader {
    receiver: mpsc::Receiver<Frame>,
    progress: Progress,
}

/// How an adapter ends a run whose delivery stalled, as its `signal(Kill)` does.
pub(crate) type Kill = Box<dyn Fn() + Send + Sync>;

/// One run's live queue under the shared drain.
pub(crate) struct LiveOutput {
    /// Set by the stall or an imposed end, before the run is signalled: no drain forwards from
    /// then on. A flag on the sending side, so closing never waits for a reader.
    closed: watch::Sender<bool>,
    /// Set only by the stall, before `closed`.
    stalled: AtomicBool,
    /// Held across one `read_output` call (design 32 § 3, *One reader at a time*).
    reader: tokio::sync::Mutex<Reader>,
    kill: Kill,
}

impl LiveOutput {
    /// A queue of `queued_chunks` chunks and the feed of each stream into it. `kill` ends the
    /// run when its delivery stalls.
    pub(crate) fn new(queued_chunks: u32, kill: Kill) -> (Arc<Self>, [Feed; 2]) {
        let capacity = usize::try_from(queued_chunks.clamp(1, MAX_QUEUED_CHUNKS)).unwrap_or(1);
        let (sender, receiver) = mpsc::channel(capacity);
        let live = Arc::new(Self {
            closed: watch::Sender::new(false),
            stalled: AtomicBool::new(false),
            reader: tokio::sync::Mutex::new(Reader {
                receiver,
                progress: Progress::default(),
            }),
            kill,
        });
        let feed = |stream| Feed {
            sender: sender.clone(),
            live: Arc::clone(&live),
            stream,
        };
        let feeds = [feed(PipeStream::Stdout), feed(PipeStream::Stderr)];
        (live, feeds)
    }

    /// Stops delivery for an imposed end; the adapter signals the run after this. Chunks queued
    /// before it stay readable.
    pub(crate) fn close(&self) {
        self.closed.send_replace(true);
    }

    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Whether delivery stopped at the stall deadline.
    pub(crate) fn stalled(&self) -> bool {
        self.stalled.load(Ordering::Acquire)
    }

    /// The stall: unless an imposed end closed the queue first, set the flag, close the queue
    /// and end the run.
    fn stall(&self) {
        let stalled = &self.stalled;
        let first = self.closed.send_if_modified(|closed| {
            if *closed {
                return false;
            }
            stalled.store(true, Ordering::Release);
            *closed = true;
            true
        });
        if first {
            (self.kill)();
        }
    }

    /// The next event, waiting at most `timeout`. `record` reads the run's record as it is once
    /// the queue is over.
    pub(crate) async fn read(
        &self,
        timeout: Duration,
        record: impl FnOnce() -> [(bool, u64); 2],
    ) -> Result<Option<OutputEvent>, ConfinementError> {
        if timeout.is_zero() {
            return Err(timeout_invalid());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let mut reader = tokio::time::timeout_at(deadline, self.reader.lock())
            .await
            .map_err(|_| read_timeout())?;
        let reader = &mut *reader;
        if !reader.progress.exhausted() {
            let mut closed = self.closed.subscribe();
            loop {
                match reader.receiver.try_recv() {
                    Ok(frame) => return Ok(Some(reader.progress.chunk(frame.stream, frame.bytes))),
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                    Err(mpsc::error::TryRecvError::Empty) => {
                        if *closed.borrow_and_update() {
                            break;
                        }
                    }
                }
                tokio::select! {
                    biased;
                    frame = reader.receiver.recv() => match frame {
                        Some(frame) => {
                            return Ok(Some(reader.progress.chunk(frame.stream, frame.bytes)));
                        }
                        None => break,
                    },
                    _ = closed.changed() => {}
                    () = tokio::time::sleep_until(deadline) => return Err(read_timeout()),
                }
            }
            reader.progress.exhaust();
        }
        Ok(reader.progress.after_queue(record(), self.stalled()))
    }
}

/// A run's live queue and its two feeds, stdout first, when `delivery` is live; none otherwise.
/// `kill` is called only for a live run.
pub(crate) fn open(
    delivery: super::OutputDelivery,
    kill: impl FnOnce() -> Kill,
) -> (Option<Arc<LiveOutput>>, [Option<Feed>; 2]) {
    match delivery.queued_chunks() {
        Some(queued_chunks) => {
            let (live, [stdout, stderr]) = LiveOutput::new(queued_chunks, kill());
            (Some(live), [Some(stdout), Some(stderr)])
        }
        None => (None, [None, None]),
    }
}

/// The run's live queue, or `session.not-pipe` for a run admitted `Recorded`.
pub(crate) fn live(live: Option<&Arc<LiveOutput>>) -> Result<&Arc<LiveOutput>, ConfinementError> {
    live.ok_or_else(not_live)
}

/// One stream's way into a run's live queue, held by that stream's drain.
pub(crate) struct Feed {
    sender: mpsc::Sender<Frame>,
    live: Arc<LiveOutput>,
    stream: PipeStream,
}

impl Feed {
    /// Queues `bytes`, which the record already holds, waiting at most the stall deadline for a
    /// slot. Once the queue is closed nothing is queued, and this returns at once.
    pub(crate) async fn forward(&self, bytes: &[u8]) {
        if bytes.is_empty() || self.live.is_closed() {
            return;
        }
        let mut closed = self.live.closed.subscribe();
        tokio::select! {
            biased;
            _ = closed.wait_for(|closed| *closed) => {}
            permit = self.sender.reserve() => {
                // A sibling stream may have stalled while this one waited for its slot.
                if let Ok(permit) = permit
                    && !self.live.is_closed()
                {
                    permit.send(Frame {
                        stream: self.stream,
                        bytes: bytes.to_vec(),
                    });
                }
            }
            () = tokio::time::sleep(STALL_DEADLINE) => self.live.stall(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn counting() -> (Kill, Arc<AtomicUsize>) {
        let kills = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&kills);
        (
            Box::new(move || {
                counted.fetch_add(1, Ordering::AcqRel);
            }),
            kills,
        )
    }

    const LONG: Duration = Duration::from_secs(5);

    // § 3 *Ordering and loss*: contiguous offsets per stream, then `None` once both feeds ended.
    #[tokio::test]
    async fn chunks_carry_contiguous_offsets_per_stream_then_none() {
        let (kill, kills) = counting();
        let (live, [stdout, stderr]) = LiveOutput::new(4, kill);
        stdout.forward(b"ab").await;
        stderr.forward(b"x").await;
        stdout.forward(b"cde").await;
        drop((stdout, stderr));
        let record = || [(false, 5), (false, 1)];
        let mut events = Vec::new();
        while let Some(event) = live.read(LONG, record).await.expect("read") {
            events.push(event);
        }
        assert_eq!(
            events,
            [
                OutputEvent::Chunk {
                    stream: PipeStream::Stdout,
                    offset: 0,
                    bytes: b"ab".to_vec()
                },
                OutputEvent::Chunk {
                    stream: PipeStream::Stderr,
                    offset: 0,
                    bytes: b"x".to_vec()
                },
                OutputEvent::Chunk {
                    stream: PipeStream::Stdout,
                    offset: 2,
                    bytes: b"cde".to_vec()
                },
            ]
        );
        assert_eq!(live.read(LONG, record).await.expect("read"), None);
        assert_eq!(kills.load(Ordering::Acquire), 0);
        assert!(!live.stalled());
    }

    // § 3 *Backpressure*: a chunk that finds no slot within the deadline stalls the run once,
    // kills it once, and the queued chunk stays readable before `Stalled`.
    #[tokio::test]
    async fn a_full_queue_past_the_deadline_stalls_and_kills_once() {
        let (kill, kills) = counting();
        let (live, [stdout, stderr]) = LiveOutput::new(1, kill);
        stdout.forward(b"one").await;
        let began = tokio::time::Instant::now();
        tokio::join!(stdout.forward(b"two"), stderr.forward(b"three"));
        assert!(began.elapsed() >= STALL_DEADLINE);
        assert_eq!(kills.load(Ordering::Acquire), 1, "one stall, one kill");
        assert!(live.stalled());
        stdout.forward(b"late").await;
        let record = || [(false, 11), (false, 5)];
        let first = live.read(LONG, record).await.expect("read");
        assert_eq!(
            first,
            Some(OutputEvent::Chunk {
                stream: PipeStream::Stdout,
                offset: 0,
                bytes: b"one".to_vec()
            })
        );
        assert_eq!(
            live.read(LONG, record).await.expect("read"),
            Some(OutputEvent::Stalled),
            "the closed queue ends without waiting for the feeds"
        );
        assert_eq!(live.read(LONG, record).await.expect("read"), None);
        assert_eq!(live.read(LONG, record).await.expect("read"), None);
    }

    // § 3 *Delivery stops*: an imposed end closes the queue; a pending forward returns at once,
    // nothing is queued after it, and no `Stalled` follows.
    #[tokio::test]
    async fn an_imposed_end_closes_the_queue_without_a_stall() {
        let (kill, kills) = counting();
        let (live, [stdout, _stderr]) = LiveOutput::new(1, kill);
        stdout.forward(b"one").await;
        let pending = stdout.forward(b"two");
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut pending)
                .await
                .is_err(),
            "the queue is full"
        );
        live.close();
        tokio::time::timeout(Duration::from_millis(500), pending)
            .await
            .expect("the close released the pending forward");
        let record = || [(false, 6), (false, 0)];
        assert!(matches!(
            live.read(LONG, record).await.expect("read"),
            Some(OutputEvent::Chunk { .. })
        ));
        assert_eq!(live.read(LONG, record).await.expect("read"), None);
        assert_eq!(kills.load(Ordering::Acquire), 0);
        assert!(!live.stalled());
    }

    // § 3 *Loss events*: `Bounded` for a truncated record whose kept bytes were all delivered,
    // stdout before stderr, and none where an offset falls short of its record.
    #[tokio::test]
    async fn bounded_follows_the_queue_only_where_every_kept_byte_was_delivered() {
        let (kill, _) = counting();
        let (live, [stdout, stderr]) = LiveOutput::new(4, kill);
        stdout.forward(b"abcd").await;
        stderr.forward(b"wx").await;
        drop((stdout, stderr));
        let record = || [(true, 4), (true, 3)];
        let mut events = Vec::new();
        while let Some(event) = live.read(LONG, record).await.expect("read") {
            events.push(event);
        }
        assert_eq!(
            events[2..],
            [OutputEvent::Bounded {
                stream: PipeStream::Stdout
            }]
        );
    }

    // § 3 *Timeouts*: zero is invalid; an elapsed deadline is a read timeout, not an event.
    #[tokio::test]
    async fn a_zero_timeout_is_invalid_and_an_elapsed_one_times_out() {
        let (kill, _) = counting();
        let (live, _feeds) = LiveOutput::new(1, kill);
        let record = || [(false, 0), (false, 0)];
        let error = live.read(Duration::ZERO, record).await.expect_err("zero");
        assert_eq!(error.code(), substrate_wire::SESSION_TIMEOUT_INVALID);
        assert_eq!(error.status(), 422);
        let error = live
            .read(Duration::from_millis(20), record)
            .await
            .expect_err("nothing came");
        assert_eq!(error.code(), substrate_wire::SESSION_READ_TIMEOUT);
        assert_eq!(error.status(), 500);
    }
}
