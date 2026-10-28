//! A run's standard input on the confinement adapters (design 32 § 2).
//!
//! A profile asks for it with [`StdinMode::Piped`](super::profile::StdinMode), which requires the
//! fact `stdin_pipe`. The embedder writes with
//! [`ConfinementAdapter::write_stdin`](super::ConfinementAdapter::write_stdin) and ends input with
//! [`ConfinementAdapter::close_stdin`](super::ConfinementAdapter::close_stdin); [`run_with_stdin`]
//! does both for a whole buffer.
//!
//! Every adapter holds its run's pipe behind one [`StdinWriter`], so the rules are the same on
//! each: at most one write in flight, nothing buffered, a cancelled write closes the pipe, and from
//! the run's observed end a write is `session.input-closed` while one pending then fails
//! `session.write-failed`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;
use tokio::sync::watch;

use super::profile::StdinMode;
use super::{
    AdmittedProfile, Command, ConfinementAdapter, ConfinementError, RunId, RunObservation,
    RunOutput,
};
use crate::DriverError;

/// The most a run's standard input takes in total: the daemon's `PIPE_MAX_INPUT_BYTES`.
pub const MAX_STDIN_BYTES: u64 = 16 * 1_024 * 1_024;

/// The most one write takes: the host's `PIPE_FRAME_BYTES`.
pub const STDIN_CHUNK_BYTES: usize = 64 * 1_024;

/// The rules every adapter's `check` applies first to a profile's streams, so a caller's mistake
/// gets the same answer on each: a piped standard input bounded at 1 to [`MAX_STDIN_BYTES`]
/// (`session.limit-unserved`, 429).
pub(crate) fn check_streams(profile: &super::ConfinementProfile) -> Result<(), ConfinementError> {
    if let StdinMode::Piped { limit_bytes } = profile.stdin
        && !(1..=MAX_STDIN_BYTES).contains(&limit_bytes)
    {
        return Err(DriverError::exhausted(
            substrate_wire::SESSION_LIMIT_UNSERVED,
            format!("A piped standard input is bounded at 1 to {MAX_STDIN_BYTES} bytes."),
            "stdin",
        )
        .into());
    }
    Ok(())
}

/// `session.not-pipe`: a stdin call on a run whose profile asked for none.
pub(crate) fn not_pipe() -> ConfinementError {
    DriverError::refused(
        substrate_wire::SESSION_NOT_PIPE,
        "The run's standard input is not a pipe.",
        "run",
    )
    .into()
}

fn input_closed() -> ConfinementError {
    DriverError::refused(
        substrate_wire::SESSION_INPUT_CLOSED,
        "The run's standard input is closed.",
        "stdin",
    )
    .into()
}

fn write_failed(detail: impl std::fmt::Display) -> ConfinementError {
    DriverError::failed(
        substrate_wire::SESSION_WRITE_FAILED,
        format!("standard input: {detail}"),
    )
    .into()
}

/// One adapter's end of a run's standard input.
#[async_trait]
pub(crate) trait StdinPipe: Send + Sync {
    /// Writes every byte, then returns.
    async fn write(&mut self, bytes: &[u8]) -> Result<(), ConfinementError>;

    /// Ends input: the child reads end of file. A second call does nothing.
    async fn close(&mut self);

    /// Ends input without awaiting, from a cancelled write's guard: where closing is synchronous
    /// it is done here, and elsewhere it is started on the current runtime, if there is one.
    fn close_now(&mut self);
}

/// The descriptor of a process the adapter spawned: the child itself, or a runtime client.
pub(crate) struct ChildPipe(Option<ChildStdin>);

impl ChildPipe {
    pub(crate) fn new(stdin: ChildStdin) -> Self {
        Self(Some(stdin))
    }
}

#[async_trait]
impl StdinPipe for ChildPipe {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), ConfinementError> {
        let Some(stdin) = self.0.as_mut() else {
            return Err(input_closed());
        };
        stdin.write_all(bytes).await.map_err(write_failed)?;
        stdin.flush().await.map_err(write_failed)
    }

    async fn close(&mut self) {
        self.0.take();
    }

    fn close_now(&mut self) {
        self.0.take();
    }
}

/// What a process an adapter spawns gets as its standard input under `mode`.
pub(crate) fn stdio(mode: StdinMode) -> std::process::Stdio {
    if mode.is_piped() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    }
}

/// The writer over a spawned process's standard input, when `mode` piped it.
pub(crate) fn child_writer(mode: StdinMode, stdin: Option<ChildStdin>) -> Option<Arc<StdinWriter>> {
    match (mode, stdin) {
        (StdinMode::Piped { limit_bytes }, Some(stdin)) => Some(StdinWriter::new(
            limit_bytes,
            Box::new(ChildPipe::new(stdin)),
        )),
        _ => None,
    }
}

/// A run's writer, or `session.not-pipe` for a run admitted without one.
pub(crate) fn piped(
    writer: Option<&Arc<StdinWriter>>,
) -> Result<&Arc<StdinWriter>, ConfinementError> {
    writer.ok_or_else(not_pipe)
}

struct State {
    pipe: Box<dyn StdinPipe>,
    /// Closed by `close`, a failed or cancelled write, or the run's end.
    closed: bool,
}

/// The one writer of a run's standard input (design 32 § 2, *One writer per run*).
pub(crate) struct StdinWriter {
    limit: u64,
    /// The bytes every write was given, counted before it writes.
    given: AtomicU64,
    state: tokio::sync::Mutex<State>,
    /// Set by a cancelled write: from then on every write is `session.input-closed`.
    poisoned: AtomicBool,
    /// Set once the adapter observed the run's end.
    ended: watch::Sender<bool>,
}

/// Armed across one write: dropped while the write is pending, it poisons the writer and closes
/// the pipe, so the child reads the part of the chunk the pipe took and then end of file, never a
/// splice.
struct Armed<'writer> {
    state: tokio::sync::MutexGuard<'writer, State>,
    poisoned: &'writer AtomicBool,
    pending: bool,
}

impl Drop for Armed<'_> {
    fn drop(&mut self) {
        if self.pending {
            self.poisoned.store(true, Ordering::Release);
            self.state.pipe.close_now();
            self.state.closed = true;
        }
    }
}

impl StdinWriter {
    pub(crate) fn new(limit: u64, pipe: Box<dyn StdinPipe>) -> Arc<Self> {
        Arc::new(Self {
            limit,
            given: AtomicU64::new(0),
            state: tokio::sync::Mutex::new(State {
                pipe,
                closed: false,
            }),
            poisoned: AtomicBool::new(false),
            ended: watch::Sender::new(false),
        })
    }

    /// Writes one chunk, returning once the pipe took every byte.
    ///
    /// # Errors
    ///
    /// `session.frame-limit` for an empty chunk or one over [`STDIN_CHUNK_BYTES`],
    /// `session.input-limit` past the run's total, `session.input-closed` after a close, a
    /// cancelled write or the run's observed end, and `session.write-failed` when the pipe refused
    /// the bytes or the run ended while the write was pending.
    pub(crate) async fn write(&self, bytes: &[u8]) -> Result<(), ConfinementError> {
        if bytes.is_empty() || bytes.len() > STDIN_CHUNK_BYTES {
            return Err(DriverError::exhausted(
                substrate_wire::SESSION_FRAME_LIMIT,
                format!("A standard input write holds 1 to {STDIN_CHUNK_BYTES} bytes."),
                "stdin",
            )
            .into());
        }
        let count = bytes.len() as u64;
        let limit = self.limit;
        if self
            .given
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |given| {
                given.checked_add(count).filter(|next| *next <= limit)
            })
            .is_err()
        {
            return Err(DriverError::exhausted(
                substrate_wire::SESSION_INPUT_LIMIT,
                format!("The run's standard input takes at most {limit} bytes."),
                "stdin",
            )
            .into());
        }
        let mut state = self.state.lock().await;
        if state.closed || self.poisoned.load(Ordering::Acquire) || *self.ended.borrow() {
            // Idempotent: a cancellation that could not close the pipe is closed here.
            state.pipe.close().await;
            state.closed = true;
            return Err(input_closed());
        }
        let mut ended = self.ended.subscribe();
        let mut armed = Armed {
            state,
            poisoned: &self.poisoned,
            pending: true,
        };
        let written = {
            let write = armed.state.pipe.write(bytes);
            tokio::select! {
                biased;
                written = write => Some(written),
                _ = ended.wait_for(|ended| *ended) => None,
            }
        };
        armed.pending = false;
        match written {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => {
                armed.state.pipe.close().await;
                armed.state.closed = true;
                Err(error)
            }
            None => {
                armed.state.pipe.close().await;
                armed.state.closed = true;
                Err(write_failed("the run ended while the write was pending"))
            }
        }
    }

    /// Ends input. Waits for a pending write; always `Ok`, however often it is called.
    pub(crate) async fn close(&self) {
        let mut state = self.state.lock().await;
        state.pipe.close().await;
        state.closed = true;
    }

    /// The adapter observed the run's end: a pending write fails, the pipe is closed, and every
    /// later write is `session.input-closed`.
    pub(crate) async fn end(&self) {
        self.ended.send_replace(true);
        self.close().await;
    }

    /// Whether the adapter observed the run's end.
    #[cfg(test)]
    pub(crate) fn ended(&self) -> bool {
        *self.ended.borrow()
    }
}

/// Starts `command` under `admitted`, writes `input` to its standard input in
/// [`STDIN_CHUNK_BYTES`] chunks, closes it, waits for the run to end and reads its output.
///
/// Returns the observation, the output and the number of bytes the pipe accepted, which is less
/// than `input.len()` when the child closed its standard input or ended first: it read what it
/// read, and that is not an error.
///
/// If this future is dropped before the run ended, the run is sent `KILL` on the current Tokio
/// runtime; with no runtime current nothing can be sent, and the run continues until it ends or
/// the embedder signals it.
///
/// # Errors
///
/// Before anything starts, and with no start record: `session.not-pipe` (422) for a profile
/// admitted without [`StdinMode::Piped`], and `session.input-limit` (429) for an `input` longer
/// than its `limit_bytes`. Then any refusal of the start, or of the adapter's calls.
pub async fn run_with_stdin(
    adapter: &Arc<dyn ConfinementAdapter>,
    admitted: &AdmittedProfile,
    command: &Command,
    input: &[u8],
) -> Result<(RunObservation, RunOutput, u64), ConfinementError> {
    let StdinMode::Piped { limit_bytes } = admitted.profile().stdin else {
        return Err(not_pipe());
    };
    if input.len() as u64 > limit_bytes {
        return Err(DriverError::exhausted(
            substrate_wire::SESSION_INPUT_LIMIT,
            format!("The input is longer than the run's {limit_bytes}-byte standard input bound."),
            "stdin",
        )
        .into());
    }
    let run = adapter.start(admitted, command).await?;
    let mut guard = KillOnDrop {
        adapter: Arc::clone(adapter),
        run: Some(run.clone()),
    };
    let mut accepted = 0_u64;
    for chunk in input.chunks(STDIN_CHUNK_BYTES) {
        match adapter.write_stdin(&run, chunk).await {
            Ok(()) => accepted += chunk.len() as u64,
            Err(error)
                if matches!(
                    error.code(),
                    substrate_wire::SESSION_WRITE_FAILED | substrate_wire::SESSION_INPUT_CLOSED
                ) =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    adapter.close_stdin(&run).await?;
    let observation = adapter.wait(&run).await?;
    guard.run = None;
    let output = adapter.output(&run).await?;
    Ok((observation, output, accepted))
}

/// Sends `KILL` to a run whose helper was dropped before the run ended.
struct KillOnDrop {
    adapter: Arc<dyn ConfinementAdapter>,
    run: Option<RunId>,
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let adapter = Arc::clone(&self.adapter);
            runtime.spawn(async move {
                let _ = adapter.signal(&run, substrate_wire::Signal::Kill).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use tokio::sync::{Notify, Semaphore};

    use super::super::{
        AdapterFacts, AdapterName, ConfinementProfile, ContractFamily, Fact, RenderedCommand,
    };
    use super::*;

    /// What a [`TestPipe`] saw, shared with the test.
    #[derive(Clone)]
    struct Probe {
        written: Arc<parking_lot::Mutex<Vec<u8>>>,
        closed: Arc<AtomicBool>,
        /// Notified when a write has entered the pipe.
        entered: Arc<Notify>,
        /// Each write takes one permit before it writes, when there is a gate.
        gate: Option<Arc<Semaphore>>,
    }

    impl Probe {
        fn open() -> Self {
            Self {
                written: Arc::default(),
                closed: Arc::default(),
                entered: Arc::new(Notify::new()),
                gate: None,
            }
        }

        /// A pipe whose writes wait until the test adds permits: a child that does not read.
        fn gated() -> Self {
            Self {
                gate: Some(Arc::new(Semaphore::new(0))),
                ..Self::open()
            }
        }

        fn writer(&self, limit: u64) -> Arc<StdinWriter> {
            StdinWriter::new(limit, Box::new(TestPipe(self.clone())))
        }
    }

    struct TestPipe(Probe);

    #[async_trait]
    impl StdinPipe for TestPipe {
        async fn write(&mut self, bytes: &[u8]) -> Result<(), ConfinementError> {
            if self.0.closed.load(Ordering::Acquire) {
                return Err(write_failed("the test pipe is closed"));
            }
            self.0.entered.notify_one();
            if let Some(gate) = &self.0.gate {
                gate.acquire().await.expect("gate").forget();
            }
            self.0.written.lock().extend_from_slice(bytes);
            Ok(())
        }

        async fn close(&mut self) {
            self.0.closed.store(true, Ordering::Release);
        }

        fn close_now(&mut self) {
            self.0.closed.store(true, Ordering::Release);
        }
    }

    fn assert_refused(error: &ConfinementError, code: &str, status: u16) {
        assert_eq!(error.code(), code, "{error}");
        assert_eq!(error.status(), status, "{error}");
    }

    // T1a, the rule itself: a piped standard input is bounded at 1 to 16 MiB.
    #[test]
    fn check_streams_bounds_a_piped_input_at_one_to_sixteen_mib() {
        let profile = |stdin| ConfinementProfile {
            stdin,
            ..ConfinementProfile::default()
        };
        for limit_bytes in [0, MAX_STDIN_BYTES + 1] {
            let error = check_streams(&profile(StdinMode::Piped { limit_bytes }))
                .expect_err("out of range");
            assert_refused(&error, substrate_wire::SESSION_LIMIT_UNSERVED, 429);
        }
        for limit_bytes in [1, MAX_STDIN_BYTES] {
            check_streams(&profile(StdinMode::Piped { limit_bytes })).expect("in range");
        }
        check_streams(&profile(StdinMode::Null)).expect("no pipe, no bound");
    }

    // § 2 *Bounded*: each write 1 to 64 KiB, the total at most the limit, counted as given.
    #[tokio::test]
    async fn a_write_holds_one_to_64_kib_and_the_writes_hold_the_limit() {
        let probe = Probe::open();
        let writer = probe.writer(10);
        assert_refused(
            &writer.write(b"").await.expect_err("empty"),
            substrate_wire::SESSION_FRAME_LIMIT,
            429,
        );
        assert_refused(
            &writer
                .write(&vec![0; STDIN_CHUNK_BYTES + 1])
                .await
                .expect_err("over a chunk"),
            substrate_wire::SESSION_FRAME_LIMIT,
            429,
        );
        writer.write(b"abcdef").await.expect("6 of 10");
        assert_refused(
            &writer.write(b"ghijk").await.expect_err("11 of 10"),
            substrate_wire::SESSION_INPUT_LIMIT,
            429,
        );
        writer.write(b"ghij").await.expect("10 of 10");
        assert_eq!(*probe.written.lock(), b"abcdefghij");
    }

    // § 2 *Ended by EOF*: close half-closes, a second close is `Ok`, a write after it is refused.
    #[tokio::test]
    async fn after_close_a_write_is_input_closed_and_close_stays_ok() {
        let probe = Probe::open();
        let writer = probe.writer(100);
        writer.write(b"one").await.expect("written");
        writer.close().await;
        assert!(probe.closed.load(Ordering::Acquire), "the pipe is closed");
        writer.close().await;
        assert_refused(
            &writer.write(b"two").await.expect_err("closed"),
            substrate_wire::SESSION_INPUT_CLOSED,
            422,
        );
        assert_eq!(*probe.written.lock(), b"one");
    }

    // § 2 *Cancellation closes*: a write dropped while pending poisons the writer and closes the
    // pipe, synchronously, in the guard's `Drop`.
    #[tokio::test]
    async fn a_cancelled_write_closes_the_pipe_and_every_later_write_is_input_closed() {
        let probe = Probe::gated();
        let writer = probe.writer(100);
        let cancelled = tokio::time::timeout(Duration::from_millis(50), writer.write(b"x")).await;
        assert!(cancelled.is_err(), "the write was pending, and was dropped");
        assert!(
            probe.closed.load(Ordering::Acquire),
            "the cancellation closed the pipe"
        );
        assert_refused(
            &writer.write(b"y").await.expect_err("poisoned"),
            substrate_wire::SESSION_INPUT_CLOSED,
            422,
        );
        writer.close().await;
        assert!(probe.written.lock().is_empty());
    }

    // § 2 *Pending writes*: close takes the same lock, so it waits for a pending write.
    #[tokio::test]
    async fn close_waits_for_a_pending_write() {
        let probe = Probe::gated();
        let writer = probe.writer(100);
        let entered = probe.entered.notified();
        let pending = tokio::spawn({
            let writer = Arc::clone(&writer);
            async move { writer.write(b"x").await }
        });
        entered.await;
        let mut closing = tokio::spawn({
            let writer = Arc::clone(&writer);
            async move { writer.close().await }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut closing)
                .await
                .is_err(),
            "close returned while a write was pending"
        );
        probe.gate.as_ref().expect("gate").add_permits(1);
        pending.await.expect("joined").expect("the write completed");
        closing.await.expect("close returned once the write did");
        assert!(probe.closed.load(Ordering::Acquire));
        assert_eq!(*probe.written.lock(), b"x");
    }

    // § 2 *The run's end*: a write pending at the observed end fails, later ones are refused,
    // and the pipe is closed.
    #[tokio::test]
    async fn the_observed_end_fails_a_pending_write_and_refuses_later_ones() {
        let probe = Probe::gated();
        let writer = probe.writer(100);
        let entered = probe.entered.notified();
        let pending = tokio::spawn({
            let writer = Arc::clone(&writer);
            async move { writer.write(b"x").await }
        });
        entered.await;
        tokio::time::timeout(Duration::from_secs(3), writer.end())
            .await
            .expect("the end released the pending write");
        assert!(writer.ended());
        let error = pending
            .await
            .expect("joined")
            .expect_err("the end won the race");
        assert_refused(&error, substrate_wire::SESSION_WRITE_FAILED, 500);
        assert!(
            probe.closed.load(Ordering::Acquire),
            "the end closed the pipe"
        );
        assert_refused(
            &writer.write(b"y").await.expect_err("ended"),
            substrate_wire::SESSION_INPUT_CLOSED,
            422,
        );
        writer.close().await;
    }

    /// Proves `stdin_pipe` and starts nothing, counting the starts it was asked for.
    struct Fake {
        facts: AdapterFacts,
        starts: AtomicUsize,
    }

    #[async_trait]
    impl ConfinementAdapter for Fake {
        fn facts(&self) -> &AdapterFacts {
            &self.facts
        }

        fn render(
            &self,
            _admitted: &AdmittedProfile,
            _command: &Command,
        ) -> Result<RenderedCommand, ConfinementError> {
            Err(super::super::unknown_run())
        }

        async fn start(
            &self,
            _admitted: &AdmittedProfile,
            _command: &Command,
        ) -> Result<RunId, ConfinementError> {
            self.starts.fetch_add(1, Ordering::AcqRel);
            Err(DriverError::failed("test.start", "the fake starts nothing").into())
        }

        async fn observe(&self, _run: &RunId) -> Result<RunObservation, ConfinementError> {
            Err(super::super::unknown_run())
        }

        async fn output(&self, _run: &RunId) -> Result<RunOutput, ConfinementError> {
            Err(super::super::unknown_run())
        }

        async fn wait(&self, _run: &RunId) -> Result<RunObservation, ConfinementError> {
            Err(super::super::unknown_run())
        }

        async fn signal(
            &self,
            _run: &RunId,
            _signal: substrate_wire::Signal,
        ) -> Result<(), ConfinementError> {
            Err(super::super::unknown_run())
        }
    }

    // T7a's refusals: `run_with_stdin` refuses a `Null` profile and an over-limit input before it
    // starts anything.
    #[tokio::test]
    async fn run_with_stdin_refuses_null_and_an_over_limit_input_before_starting() {
        let fake = Arc::new(Fake {
            facts: AdapterFacts::new(
                AdapterName::Linux,
                None,
                &[(ContractFamily::ProcessRun, true)],
                &[(Fact::StdinPipe, true)],
            ),
            starts: AtomicUsize::new(0),
        });
        let adapter: Arc<dyn ConfinementAdapter> = fake.clone();
        let command = Command {
            argv: vec!["/bin/cat".into()],
            env: std::collections::BTreeMap::new(),
        };
        let null = adapter
            .admit(&ConfinementProfile::default())
            .expect("admitted");
        let error = run_with_stdin(&adapter, &null, &command, b"abc")
            .await
            .expect_err("not a pipe");
        assert_refused(&error, substrate_wire::SESSION_NOT_PIPE, 422);
        let piped = adapter
            .admit(&ConfinementProfile {
                stdin: StdinMode::Piped { limit_bytes: 4 },
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = run_with_stdin(&adapter, &piped, &command, b"abcde")
            .await
            .expect_err("over the limit");
        assert_refused(&error, substrate_wire::SESSION_INPUT_LIMIT, 429);
        assert_eq!(fake.starts.load(Ordering::Acquire), 0, "nothing started");
        let error = run_with_stdin(&adapter, &piped, &command, b"abcd")
            .await
            .expect_err("the fake starts nothing");
        assert_eq!(error.code(), "test.start");
        assert_eq!(
            fake.starts.load(Ordering::Acquire),
            1,
            "within the limit, it starts"
        );
    }
}
