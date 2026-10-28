# Design 32: standard input and live output on the confinement adapters

Status: proposed, 2026-10-28, revised after review passes 1 and 2 the same day (§ 13). § 11 names
the stories. It changes no wire byte and no contract bundle: the adapters are substrate-host's
confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). No code before acceptance (invariant 8).

Two asks from a consumer that launches children through substrate-host (its `run_child` with
standard input, and its `spawn_child`):

1. A confinement adapter run takes standard input. Today every adapter gives the run `/dev/null`.
2. A run's output reaches the embedder while the run runs. Today it is read after the run ends.

The consumer gets both today only from a raw-pipe session on `HostDriver` (`start_pipe_session`,
`write_pipe_session`, `read_pipe_session`, `close_pipe_session_input`), which bypasses the adapter
and so the confinement contract: no `systemd`, `macos` or `oci`, no adapter admission or audit,
and none of design 29's adapter-only rules (the registered-submodule refusal under a scoped
workspace, shipped in 0.14.0). This design puts the same two capabilities behind the adapter
contract, reusing the session machinery and its decided bounds rather than inventing new ones.

## 1. Evidence: what the tree does at `dc75cbd5`

The rows from "the host keeps a raw pipe's `ChildStdin`" on were added in review pass 2 and read on
this design's branch.

| what | where |
|---|---|
| every adapter spawns with `stdin(Stdio::null())`: `local`, `macos`, `systemd` around `local`, and the `oci` runtime client (`start --attach`) | `local.rs` `start_run`, `macos.rs` (the confined command), `systemd.rs` (the run's spawn), `oci.rs` `start_checked` |
| `linux` starts through `HostDriver::start_exec_for_adapter`, which reaches `start_inner` with no pipe settings, so the child channel is `ChildChannel::Quiet` (stdin `/dev/null`, output captured) | `crates/substrate-host/src/lib.rs`, `crates/substrate-host/src/process.rs` |
| `pod` renders `"stdin": false, "tty": false` and reads the log once the container terminated, `limitBytes` = bound + 1. The kubelet's log merges stdout and stderr into one stream (design 28) | `pod.rs` (the container spec, `read_log`) |
| `local`, `macos`, `systemd` around `local` and `oci` drain both pipes into one shared `Captured` through `output_retention::drain`, reading 16 KiB at a time; `output()` while running returns a snapshot. `linux` reports output only once the run ended | `crates/substrate-host/src/output_retention.rs`; design 29 § 6 |
| the host already serves live stdin and output for sessions: `ChildChannel::Pipes`, `write_pipe` (frame ≤ `frame_limit`, total ≤ `input_limit`, refusals `session.frame-limit`, `session.input-limit`, `session.input-closed`, `session.write-failed`), `close_pipe_input` (half-close, idempotent), `read_pipe` (`session.read-timeout`, `session.timeout-invalid`) | `process.rs` |
| `write_pipe` holds the stdin mutex across `write_all` and `flush`, and `close_pipe_input` takes the same mutex, so a close waits behind a write the child does not read. A `write_pipe` future dropped mid-frame may have written part of it, and nothing records that | `process.rs` `write_pipe`, `close_pipe_input` |
| live output is a bounded queue of `PipeFrame { stream, bytes }` and nothing else: frames ≤ `PIPE_FRAME_BYTES` (64 KiB), at most `PIPE_QUEUED_FRAMES` (16); a frame not reserved within `PIPE_OUTPUT_STALL_TIMEOUT` (1 s) sets the execution's `pipe_backpressure` flag, records `session.output-backpressure` and ends the tree; the drain continues so the child never blocks | `observation.rs` `PipeFrame`; `process.rs` `drain_capped`, `forward_frame`; [ADR 0031](../../adr/0031-live-output-backpressure-has-a-stall-deadline.md), which superseded ADR 0016 |
| `drain_capped` reads 8 KiB at a time and forwards each read as one frame, so a frame is at most 8 KiB in practice | `process.rs` `drain_capped` |
| with a live sender, `drain_capped` keeps `Head`, but the record is not the delivered bytes in two cases: a raw-pipe drain passes `mark_truncation = true`, so a truncated record's last 31 bytes are replaced by `TRUNCATION_MARKER`; and after a stall it keeps filling the record up to the bound while forwarding has stopped. The pty path passes `mark_truncation = false` | `process.rs` `drain_capped` and its three callers |
| the daemon caps a session's total input at `PIPE_MAX_INPUT_BYTES` (16 MiB) | `crates/substrate-daemon/src/app.rs` |
| every adapter tracks at most 128 runs | `LOCAL_MAX_TRACKED_RUNS`, `SYSTEMD_MAX_TRACKED_RUNS`, `POD_MAX_TRACKED_RUNS`, `HostConfig::max_tracked_execs` |
| `oci`'s open reads the runtime's flavor and version (`RuntimeProbe`) | `oci.rs` `parse_info` |
| public and not `#[non_exhaustive]`: `ConfinementProfile` (public fields, `Default`), `ConfinementError`, `RunObservation`, and the generated `Fact` (serde `Serialize` and `Deserialize`); `FACT_ORDER: [Fact; 11]` and `FAMILY_ORDER: [ContractFamily; 10]` are public fixed-length arrays | `confinement/mod.rs`, `confinement/profile.rs`, `confinement/facts.rs`, `crates/substrate-host/src/generated/confinement.rs`, `crates/substrate-host/tests/confinement-public-items.txt` |
| the host keeps a raw pipe's `ChildStdin` in `PipeState` for as long as it tracks the exec; nothing drops it when the run ends | `process.rs` `start_inner` (the `PipeState` it builds) |
| `read_pipe` holds the output receiver's lock around one `recv` only | `process.rs` `read_pipe` |
| a signal, a timeout, the CPU budget, an aperture ceiling or the output bound of an unrecorded or pty run closes the live receiver before reconciliation. `forward_frame` then returns `false` for every later chunk while `drain_capped` keeps filling the record; `pipe_backpressure` stays `false` and nothing else records that delivery stopped | `process.rs` `signal` (the receiver closed before signalling), the run loop's ends and `close_live_output`, `forward_frame` |
| there is no per-stream bound flag: `run_child` gives both drains of a recorded run one shared `AtomicBool` | `process.rs` `run_child` (`stdout_bound`, `stderr_bound`) |
| `RunObservation` states an end as `exit.code` or `exit.signal`; it does not say which bound ended a run | `confinement/mod.rs` `RunObservation`, `RunExit`; `linux.rs` `observation` |
| a `local` run ends only after both output readers reached end of file, so a descendant holding stdout or stderr keeps the run running | `local.rs` `start_run` (the reaping task) |
| `systemd`'s wrapper forwards each trait method to its inner adapter by hand; a method it does not forward answers the trait's default | `systemd.rs` (the wrapper's `start` to `signal`) |
| every lane runner compares the adapter's proved facts with the scenario's open step exactly. The portable, linux-delegated and macos runners pin their scenario counts, `PORTABLE_DECLARED` and `PORTABLE_EXECUTED`, `LINUX_DELEGATED`, `MACOS_DECLARED` with `MACOS_S1` and `MACOS_S2`, and the macos runner asserts that every macos scenario is `macos-s1-*`, `macos-s2-*` or `absent-*`. The oci and pod runners compare with `lane_size`. `macos.yml` pins the macos test binary's `15 passed` | `crates/substrate-host/tests/support/mod.rs` (`open`), `ess_confinement_local.rs`, `ess_confinement_linux.rs`, `ess_confinement_macos.rs`, `ess_confinement_oci.rs`, `ess_confinement_pod.rs`, `.github/workflows/macos.yml` |

The wire already serves both capabilities for the host driver: the session family
(`/v1/sessions`, [ADR 0007](../../adr/0007-protocol-processes-use-raw-pipe-sessions.md), ADR 0028)
in `crates/substrate-daemon/src/app/sessions.rs`.

## 2. Standard input: the adapter surface

### Options

| | option | cost |
|---|---|---|
| A | the whole input in `Command` at start (`stdin: Option<Vec<u8>>`), written and closed by the adapter | serves `run_child` only, not a long-lived child (`spawn_child`, a `cat-file --batch` reader). `Command` is a public struct with no `Default`, so every literal breaks |
| **B** | **a profile mode plus two trait methods**: the profile asks for a piped stdin and its total bound; `write_stdin`, `close_stdin` on the run; a free helper writes a whole buffer and closes | serves both asks; admission refuses by fact name before any start; `Command` is unchanged |
| C | hand out the child's `ChildStdin` | ties the API to Tokio pipes. `linux` writes through the host driver, `oci` through the runtime client, so no adapter but `local` holds the child's descriptor |

**Choice: B.**

```rust
// confinement/profile.rs
pub struct ConfinementProfile {
    // ... every existing field unchanged ...
    /// The run's standard input (design 32). `Null`, the default, is `/dev/null`, as before.
    pub stdin: StdinMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum StdinMode {
    #[default]
    Null,
    /// A pipe the embedder writes with `write_stdin` and ends with `close_stdin`.
    Piped { limit_bytes: u64 },
}

// confinement/mod.rs, on `ConfinementAdapter`, each with a default body that answers
// `session.not-pipe`, so an adapter that does not serve it needs no code.
async fn write_stdin(&self, run: &RunId, bytes: &[u8]) -> Result<(), ConfinementError>;
async fn close_stdin(&self, run: &RunId) -> Result<(), ConfinementError>;

// confinement/mod.rs, a free function: start, write `input` in chunks, close, wait, read.
// It takes the `Arc` that `confinement::open` returns, so its cancellation guard can own a clone.
pub async fn run_with_stdin(
    adapter: &Arc<dyn ConfinementAdapter>,
    admitted: &AdmittedProfile,
    command: &Command,
    input: &[u8],
) -> Result<(RunObservation, RunOutput, u64), ConfinementError>;
```

### Rules

- **Bounded.** `limit_bytes` is 1 to `MAX_STDIN_BYTES` (16 MiB, the daemon's
  `PIPE_MAX_INPUT_BYTES`; a daemon unit test holds the two equal). Each write is 1 to
  `STDIN_CHUNK_BYTES` (64 KiB, the host's `PIPE_FRAME_BYTES`). Out of range at admission:
  `session.limit-unserved` (429). An empty or oversized write: `session.frame-limit` (429). A write
  past the total: `session.input-limit` (429). The bytes counted are the bytes the call was given,
  before it writes.
- **Ended by EOF.** `close_stdin` half-closes: the child reads end of file, and output and `wait`
  are unaffected. A second `close_stdin` returns `Ok`. A write after it is `session.input-closed`
  (422). What the run's end does to stdin is *The run's end* below.
- **No buffering.** A write goes straight to the pipe the adapter holds and returns once that pipe
  took every byte. `Ok` means the pipe took the bytes, not that the child read them. Under `oci` the
  pipe is the runtime client's standard input: `Ok` means the client took the bytes, and the client
  forwards them to the container; the oci lane proves that every byte and the end of file arrive
  (T15). At most one write is in flight per run: a second `write_stdin` on the same run waits for
  the first. A child that does not read blocks the writer, not the host. A failed write (the child
  closed its stdin or ended) is `session.write-failed` (500) and never reported as delivered.
- **One writer per run, shared by every adapter.** A `pub(crate)` adapter-side `StdinWriter` in
  `confinement/mod.rs` holds, per run, a `tokio::sync::Mutex` that every write and close takes, a
  `poisoned: AtomicBool` and an `ended: AtomicBool`. A write takes the lock, answers
  `session.input-closed` if `poisoned` or `ended` is set, arms a drop guard, writes the chunk
  through the adapter's pipe, and disarms the guard. Under `linux` the pipe is the host driver's
  `write_pipe` and `close_pipe_input` by exec id; elsewhere it is the `ChildStdin` of the process
  the adapter spawned (the child, or the `oci` runtime client).
- **Cancellation closes.** A `write_stdin` future dropped while its guard is armed may have
  written part of its chunk. The guard's `Drop` sets `poisoned` synchronously and closes the pipe
  where that is synchronous: it owns the lock guard over the `ChildStdin` and drops it (`local`,
  `systemd` around `local`, `macos`, `oci`). Under `linux` closing is async, and `Drop` cannot
  await: it calls `tokio::runtime::Handle::try_current()` and, where a runtime is current, spawns
  `close_pipe_input` on it with a clone of the driver handle and the exec id. Where none is current
  (the future was dropped outside a runtime), it spawns nothing: `tokio::spawn` would panic there.
  In both cases the next write or close, holding the lock and seeing `poisoned`, calls
  `close_pipe_input` first (it is idempotent), and the run's end closes it as below; until one of
  them happens, a child under `linux` without a current runtime at the cancellation does not yet
  read end of file. After a cancellation every write is `session.input-closed`, and `close_stdin`
  returns `Ok`. The child reads the part of the chunk the pipe took and then end of file: a prefix,
  never a splice.
- **Pending writes.** `write_stdin` has no deadline of its own: the embedder bounds it by
  cancelling it, which closes stdin as above. `close_stdin` takes the same lock, so it waits while
  a write is pending; an embedder that wants to close now cancels the pending write, after which
  `close_stdin` returns `Ok` at once.
- **The run's end.** The run has ended for its stdin once the adapter has observed the end: the
  same moment from which `wait` returns and `observe` reports `Ended` or `Unknown`. At that moment
  the adapter sets `ended` and closes its pipe (drops the `ChildStdin`, or calls `close_pipe_input`
  under `linux`, because the host never drops a raw pipe's stdin at run end). From then on a write
  is `session.input-closed` (422) and `close_stdin` returns `Ok`. A write pending at that moment
  returns `session.write-failed` (500): the writer races each chunk against the observed end, and
  when the end wins it drops the pipe. This matters where the adapter does not prove
  `whole_tree_kill` (`local`, `macos`): a descendant that left the run's process group can still
  hold descriptor 0 after the run ended, and without the race it would hold the writer forever.
  Where the run is a cgroup, its end means every holder is gone and the write fails on its own. A
  write that started before the end and finished before the adapter observed it may have returned
  `Ok` with bytes no process read: `Ok` never meant read.
- **No rate bound beyond the pipe.** The writer is the embedder, which already chooses the argv;
  host memory is bounded by the one chunk in flight, and the child's read rate paces the writer.
  The reserved `rate_bounds` field (ADR 0044 decision 5) stays reserved.
- **Wrong run.** `write_stdin` or `close_stdin` on a run admitted with `Null` is `session.not-pipe`
  (422). An unknown run is the adapters' existing not-found.
- **`run_with_stdin`** refuses before it starts anything, writing no start record: a profile
  admitted with `Null`, `session.not-pipe` (422); an `input` longer than `limit_bytes`,
  `session.input-limit` (429); and, once `output_live` exists (§ 11, story 4), a profile admitted
  with `OutputDelivery::Live`, `confinement.live-output-unread` (422), because it reads no events,
  so a `Live` run would stall once its queue filled and be killed after the 1 s deadline. Once
  started, it holds a guard over a clone of the adapter `Arc` and the run id. If the helper's
  future is dropped before the run ended, the guard spawns `signal(run, Kill)` through
  `Handle::try_current()`, so a cancelled helper does not leave its run running; where no runtime
  is current it can spawn nothing, and the run continues until it ends on its own or the embedder
  signals it. The stdin write in flight is closed by its own guard as above. Under `Recorded` it
  writes in 64 KiB chunks while it waits, and stops writing without failing when the child closed
  its stdin or ended first (`session.write-failed` or `session.input-closed`): the child read what
  it read. It returns the number of bytes the pipe accepted. Under `Recorded` the adapter drains
  output on its own, so writing all input before reading output cannot deadlock. Full duplex under
  `Live` needs a concurrent reader: the embedder calls `write_stdin` and `read_output` from two
  tasks, which the trait allows, since both take `&self` and may be in flight on one run at once.
- **No terminal.** Stdin is a pipe, never a pty. The pty mode ([ADR 0019](../../adr/0019-pty-is-a-second-session-mode.md),
  [design 13](13-pty-sessions.md)) stays a daemon session mode.

The per-call refusals reuse the session codes of `substrate_wire` because `linux` passes the host
driver's answers through unchanged; a second spelling for the same refusal would let a client branch
on the adapter (invariant 4). They are answers to a call, not admissions or starts, so they write no
audit record, as `signal` errors write none. This design's two new codes,
`confinement.live-output-retention-invalid` and `confinement.live-output-unread`, ride the existing
`ConfinementError::Driver` variant with class `Refused` (422), so `ConfinementError` gains no
variant.

## 3. Live output: the adapter surface

### Options

| | option | cost |
|---|---|---|
| A | poll `output()` while running | `linux` has no bytes until the end; a snapshot under `Tail` loses bytes between polls; no ordering statement; copying the whole kept record per poll |
| **B** | **a bounded per-run queue of chunks, read with `read_output(run, timeout)`, terminal backpressure after the stall deadline (ADR 0031)** | the session contract, already decided and implemented in `drain_capped`; ordered and lossless per stream up to the run's end |
| C | lossy live view: drop chunks when the queue is full, count them | corrupts a line protocol (`spawn_child`'s consumers frame JSON lines); a second backpressure policy beside ADR 0031's |
| D | an unbounded channel | unbounded host memory; refused by the safety envelope's bounded output draining |

**Choice: B.**

```rust
pub struct ConfinementProfile {
    // ...
    /// How output reaches the embedder (design 32). `Recorded`, the default, is the record only.
    pub output_delivery: OutputDelivery,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputDelivery {
    #[default]
    Recorded,
    /// Also delivered while the run runs, at most `queued_chunks` chunks waiting.
    Live { queued_chunks: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Bytes of one stream; `offset` is how many bytes of that stream came before them.
    Chunk { stream: PipeStream, offset: u64, bytes: Vec<u8> },
    /// The stream's delivered bytes reached its output bound; later bytes were drained and not
    /// delivered.
    Bounded { stream: PipeStream },
    /// No chunk was taken within the stall deadline: delivery stopped and the run was ended
    /// (`session.output-backpressure`).
    Stalled,
}

// On `ConfinementAdapter`, default body `session.not-pipe`:
/// The next event, waiting at most `timeout`; `None` once both streams ended and every event
/// was read.
async fn read_output(&self, run: &RunId, timeout: Duration)
    -> Result<Option<OutputEvent>, ConfinementError>;

// On `ConfinementAdapter`, default body `false`:
/// Whether this run's live delivery stopped at the stall deadline. `false` for a run that did
/// not stall, a run admitted `Recorded`, and a run this adapter does not track (or no longer does).
fn output_stalled(&self, run: &RunId) -> bool;
```

`PipeStream` (`Stdout`, `Stderr`) is the host's existing type, re-exported from `confinement`. The
wire's `OutputStream` is a different type and is not reused.

### Rules

- **Bounded.** `queued_chunks` is 1 to `MAX_QUEUED_CHUNKS` (16, the host's `PIPE_QUEUED_FRAMES`);
  outside it, `session.limit-unserved` (429) at admission. A chunk holds 1 to 64 KiB, and its
  boundaries carry no meaning. Each stream delivers at most its existing output bound (1 MiB, or
  `io_bounds.output_bytes` where `limits` is served). No bound changes.
- **Chunk sizes in this tree.** A chunk is one read of the adapter's drain: at most 8 KiB under
  `linux` and `systemd` around `linux` (`drain_capped`'s read buffer), at most 16 KiB under
  `local`, `systemd` around `local`, `macos` and `oci` (`output_retention::drain`'s). The contract
  counts chunks, not bytes: a run stalls when `queued_chunks` chunks wait unread past the
  deadline, which is anywhere from `queued_chunks` bytes to `queued_chunks` × 64 KiB. In this tree
  that is at most 16 × 8 KiB = 128 KiB under `linux` and 16 × 16 KiB = 256 KiB elsewhere. Coalescing
  reads into 64 KiB chunks is rejected: it would hold back a line the child already wrote, waiting
  for more bytes.
- **Backpressure is terminal after the stall deadline**, as ADR 0031 decides for sessions: the
  adapter waits at most `PIPE_OUTPUT_STALL_TIMEOUT` (1 s) for a queue slot, then stops delivery and
  ends the run as its `signal(Kill)` does (whole tree where the adapter proves `whole_tree_kill`,
  the process group or container otherwise). The drain continues, so the child never blocks on its
  pipes. ADR 0031 decided this for a daemon session, whose reader is a client on the wire. This
  design applies the same deadline and the same kill to an embedder reading in its own process: the
  rule and its bounds are ADR 0031's, and the reader it now covers is new (§ 10, D4 and Q5).
- **Delivery stops when the run is ended for it.** A run ends naturally when its process exits on
  its own (an exit code, or a signal the adapter did not send). Every other end is imposed: the
  embedder's `signal`, a timeout, the CPU budget, an aperture ceiling, an output bound that ends
  the run, or the stall. On an imposed end the adapter stops delivery at once: under `linux` the
  host closes the live receiver before it reconciles (§ 1), and the shared drain does the same by
  closing its queue before the adapter signals. Chunks queued before that stay readable, in order.
  The drain keeps reading the pipes and keeps filling the record within its bound, so the record,
  not the delivered chunks, is the complete account of what the run wrote: the chunks are a prefix
  of each stream's record. No event is synthesized for an imposed end other than the stall:
  `read_output` returns the queued chunks, then `Stalled` if the end was the stall, then `None`.
  The observation states the end as it does for a `Recorded` run, by `exit.signal`; which bound
  imposed it is not on `RunObservation` today, and this design does not add it. `output_stalled`
  alone tells the stall apart.
- **Loss events are synthesized after the queue, not queued.** The host queue carries only chunks,
  and a stall happens exactly when it is full, so a queued `Stalled` would have no slot.
  `read_output` therefore returns the queued chunks in order and counts each stream's delivered
  `offset` itself. Once the queue is exhausted (every drain ended and every chunk was read) the run
  has ended, and it returns, in this order and each at most once: `Bounded { Stdout }` if the
  adapter's delivered stdout offset equals the stdout output bound and the run's final `output()`
  has `stdout_truncated`; `Bounded { Stderr }` likewise with `stderr_truncated`; `Stalled` if the
  stall flag is set; then `None` from then on. The bound is read from the delivered offset and the
  final record, not from a drain flag, because the host has no per-stream bound flag (§ 1). The
  stall flag is the execution's `pipe_backpressure` under `linux`, read through a new `pub(crate)`
  `HostDriver` accessor by exec id, and a flag in the adapter's run state under the shared drain.
- **One reader at a time.** The adapter holds a per-run reader lock (`tokio::sync::Mutex`) across
  one host read and the offset update that follows it, and across the synthesized events. The
  host's own lock covers only the `recv` (§ 1), so without it two concurrent `read_output` calls
  could return chunks with swapped offsets or the same synthesized event twice. Concurrent calls
  are served one after the other, each with the next event.
- **Ordering and loss.** Within one stream: in order, contiguous (`offset` + `bytes.len()` is the
  next chunk's `offset`), no duplicate, no gap. Between stdout and stderr: no order is promised.
  A pipe pair has none to observe. On a natural end every byte within the bound is delivered, and
  crossing the bound is the named `Bounded`. On an imposed end delivery stops as stated above, the
  stall is the named `Stalled`, and the record holds the rest. A loss event comes after every
  chunk and before `None`, never in place of a chunk.
- **The record holds the delivered head.** With `Live`, `output()` after the run holds each
  stream's head within the bound, and for every chunk `stdout[offset..offset + len] == bytes` (and
  likewise for stderr): each stream's record begins with exactly the bytes delivered. On a natural
  end the record equals them. On an imposed end the drain keeps filling the record up to the bound
  while delivery has stopped, so the record can hold bytes past the last chunk. No truncation
  marker is written into a `Live` record: under `linux` the adapter's live drain passes
  `mark_truncation = false`, as the pty path does, so the marker never replaces the last 31 bytes
  of a delivered head; `stdout_truncated` and `KeptOutput` say what is missing. `KeptOutput`
  reports `Head`. A profile asking `Live` together with `Tail` or `HeadAndTail` is refused
  `confinement.live-output-retention-invalid` (422), because the record could then not begin with
  the delivered bytes (§ 10, Q2). A `Recorded` record is unchanged, the marker included.
- **Unread output.** A `Live` run whose embedder does not read stalls once `queued_chunks` chunks
  wait past the deadline, and ends `Stalled`. An embedder that only wants the record asks for
  `Recorded`, which never stalls. Events still queued when the run ends stay readable until the
  adapter forgets the run (its existing 128-run tracking bound). Per run, the queue adds at most
  `queued_chunks` chunks to the record's memory: 1 MiB by the contract's 64 KiB chunk, and in this
  tree at most 256 KiB. Over 128 tracked runs that is at most 128 MiB by the contract and 32 MiB in
  this tree, beside the records' existing bound.
- **Timeouts.** A zero timeout is `session.timeout-invalid` (422), and an elapsed one
  `session.read-timeout` (500), as `read_pipe`. `read_output` on a `Recorded` run is
  `session.not-pipe` (422).
- **`wait` and `output` are unchanged.** `wait` returns when the run ended, whether or not its
  events were read. An embedder that did not read the events learns of a stall from
  `output_stalled`, which also tells a stall kill apart from a `signal(Kill)` the embedder sent.

## 4. Facts, families and admission

| profile field | implies family | implies fact |
|---|---|---|
| `stdin: Piped { .. }` | `process_run` | `stdin_pipe` |
| `output_delivery: Live { .. }` | `output_read` | `output_live` |

**Options.** Two new contract families (`input_write`, `output_stream`) would change atlas ADR 0109
decision 8's closed family list and `FAMILY_ORDER`. Host-local refusals only, with no fact, as
design 29 did for scoped write, would leave an embedder unable to choose an adapter by its facts
before admitting, which is what a consumer selecting by facts needs. **Choice: two facts**,
appended to `Fact` after `image_digest_pinned`, `stdin_pipe` first, so `FACT_ORDER` names every
existing fact first and no existing refusal changes. A profile requiring one the adapter did not
prove is `confinement.fact-unserved` (501) naming it, at admission, with its `profile_refused`
audit record. The admission record's `facts` lists them, so the audit says the run had a pipe. The
stdin bytes and output chunks are never audited or logged.

The range checks (`limit_bytes`, `queued_chunks`) and the retention conflict sit in one shared
`pub(crate) fn check_streams(&ConfinementProfile)` every adapter's `check` calls first, as
`check_workspace_access` is shared, so a caller's mistake gets the same answer on every adapter.
The stdin story creates it with the `limit_bytes` check; the live-output story adds the
`queued_chunks` check and the retention conflict (§ 11).

### Public API compatibility

Three breaks, each one `CHANGELOG.md` line under the next minor. Every consumer pins an exact
revision and the crate is 0.x, so none is hidden behind a version a consumer did not choose:

1. **`Fact` gains `StdinPipe` and `OutputLive`.** It is generated from the ESS domain, is not
   `#[non_exhaustive]` and derives `Serialize` and `Deserialize`. An exhaustive `match` breaks, and a
   consumer on an older revision fails to deserialize an `AdapterFacts` or an audit record that
   names either fact. It stays exhaustive: it is generated, and changing the generator is not this
   design.
2. **`FACT_ORDER` changes type** from `[Fact; 11]` to `[Fact; 12]` with `stdin_pipe` and to
   `[Fact; 13]` with `output_live`: code that names the array type or relies on its length breaks.
   `FAMILY_ORDER` (`[ContractFamily; 10]`) does not change, because no family is added.
3. **`ConfinementProfile` gains `stdin` and `output_delivery`.** It is not `#[non_exhaustive]`, so
   a struct literal without `..ConfinementProfile::default()` and an exhaustive destructuring
   pattern break. Marking it `#[non_exhaustive]` now would break every literal outside the crate,
   so it is not done here.

If the two pairs of § 11 land in different releases, each release carries its half of each break.
Not breaks: the four trait methods carry default bodies, so an implementor outside the crate
compiles unchanged. `StdinMode` and `OutputDelivery` are `#[non_exhaustive]` from the start, as
`OutputEvent` is, so a later mode or event is additive. `run_with_stdin` is new, so its `Arc`
parameter breaks nothing. `ConfinementError`, `RunObservation`, `RunOutput`, `Command` and
`AdapterFacts` keep their shape.

## 5. Per-adapter feasibility

| adapter | `stdin_pipe` | `output_live` | mechanism |
|---|---|---|---|
| `local` | proved | proved | `Stdio::piped()` for stdin, held by the shared `StdinWriter`; `output_retention::drain` gains an optional live queue that it fills after the record, with the stall flag in the run state; the stall and `signal` close the queue, then kill the process group |
| `systemd` around `local` | proved | proved | its own spawn, the same code as `local`; the stall kills the run's cgroup |
| `linux` | proved with the floor | proved with the floor | the host driver's session path. `AdapterStart` gains `stdin: bool` and `live: Option<queue>` (`pub(crate)`). `ChildChannel::Pipes` without a live sender is new: stdin piped, output recorded through `drain_capped` with no sender, so `Piped` + `Recorded` never stalls. With a live sender the adapter's drain passes `mark_truncation = false`. `write_pipe`, `close_pipe_input` and `read_pipe` serve the adapter by exec id behind the `StdinWriter` and the reader lock, and a `pub(crate)` accessor reads `pipe_backpressure`; there is no bound flag to read (§ 3). The adapter calls `close_pipe_input` when it observes the run's end (§ 2). Sessions keep their path unchanged |
| `systemd` around `linux` | as `linux` | as `linux` | delegates to `linux`. Its wrapper forwards all four new methods, `write_stdin`, `close_stdin`, `read_output` and `output_stalled`, to the inner adapter, as it forwards `start` to `signal` today; a method left unforwarded would answer the trait's default while the fact is declared (T13) |
| `macos` | proved once the macos lane case passes | the same | the same `Captured` path as `local`. Seatbelt rules govern opens by path, and the run inherits descriptors 0 to 2. The macos lane proves that a confined `cat` reads and writes them before the fact is declared (ADR 0023's rule: nothing published unproved) |
| `oci` | proved only for pinned runtime and version pairs | proved | `create --interactive` and `start --attach --interactive` with the runtime client's stdin piped; output is already the attached client's two pipes. A write returns once the client's pipe took the bytes (§ 2). The runtime's detach sequence (`ctrl-p ctrl-q`) must not eat input bytes: the adapter disables it where the runtime has an option for it. `open` declares `stdin_pipe` only when the probe's flavor and version are a pair in `OCI_STDIN_PINNED`, a list that holds only pairs on which the oci lane wrote the bytes `0x10 0x11` through the container and read back every byte and the end of file (T15). On any other pair the fact is absent and `Piped` is refused `confinement.fact-unserved` naming `stdin_pipe` (invariant 3; atlas ADR 0109 decision 2). A probe at open, writing those bytes through a throwaway container, is a later option and not this design |
| `pod` | **not served** | **not served** | stdin needs `stdin: true` and `stdinOnce: true` in the Pod and the `pods/attach` subresource on the Role: new authority (attach to any Pod in the namespace) and a new streaming client. Live output is deferred: the log endpoint merges stdout and stderr into one stream, so a chunk could name no true `PipeStream`, and labelling the merged stream `Stdout` would misstate it. It waits for a design that states merged-stream delivery (an event or fact that says the streams are one), written when a consumer needs it. Both refused `confinement.fact-unserved` |

§ 11 orders the work: standard input on every adapter above first, then live output. Within each,
the `local`, `linux` and `systemd` parts are what the consumer runs on.

## 6. The ESS confinement domain and suite

The spec changes are split per fact, one spec story each, and each lands together with the adapter
story that proves its fact (§ 11).

Changes in `spec/confinement/confinement.yaml`, the stdin spec story first, then the live spec
story:

- `Fact` gains `stdin_pipe`, then `output_live`, with a comment line each, after
  `image_digest_pinned`.
- `AdmitProfile` gains the outcome `fact-unserved-stdin-pipe`, then `fact-unserved-output-live`,
  after `fact-unserved-image-digest-pinned`.
- The declared sets above `OpenAdapter` gain each fact as § 5 states, with its source. The `oci`
  set states what the oci lane's pinned runtime yields. The `pod` set does not change.

The model does not see `limit_bytes`, `queued_chunks` or the retention conflict: like design 29's
refusals, they are raised by an adapter's `check`, which `AdmitProfile` does not model, and are held
by the Rust cases of § 9. Design 29 § 10 records that gap, and this design does not close it.

New authored scenarios, named by the lane rule of `spec/confinement/README.md`. The macos ones use
a new `macos-io-` stage beside `macos-s1-` and `macos-s2-`:

| stdin spec story | live spec story | lane |
|---|---|---|
| `local-admits-stdin-pipe-on-portable` | `local-admits-output-live-on-portable` | portable |
| `linux-admits-stdin-pipe-on-delegated` | `linux-admits-output-live-on-delegated` | delegated |
| `systemd-local-admits-stdin-pipe-on-delegated` | `systemd-local-admits-output-live-on-delegated` | delegated |
| `systemd-linux-admits-stdin-pipe-on-delegated` | `systemd-linux-admits-output-live-on-delegated` | delegated |
| `model-stdin-pipe-unserved-on-interpreted` (an adapter set without the fact, refused by name) | `model-output-live-unserved-on-interpreted` | interpreted |
| `macos-io-admits-stdin-pipe-on-macos` | `macos-io-admits-output-live-on-macos` | macos |
| `oci-admits-stdin-pipe-on-oci` | `oci-admits-output-live-on-oci` | oci |
| `pod-stdin-pipe-unserved-on-pod` | `pod-output-live-unserved-on-pod` | pod |

**Each spec story edits the suite and every count pinned on it, and no adapter story does.** Every
lane compares the adapter's real facts with the scenario's open step exactly
(`crates/substrate-host/tests/support/mod.rs`), so adding a fact to an adapter also edits the open
step of every authored scenario that states that adapter's declared set (at `dc75cbd5`, 11 files
name `local`, 11 `linux` and 7 `systemd`, besides every `macos` and `oci` one). Each spec story
makes, for its fact: the fact, the outcome, every open-step edit, its eight scenarios, the
regenerated `spec/confinement-suite.json` and `Fact` (`cargo xtask check-ess`), the edits that keep
the regenerated `Fact` compiling (`FACT_ORDER` and any exhaustive `match` on `Fact`; no adapter
proof changes), the `Fact` and `FACT_ORDER` lines of `tests/confinement-public-items.txt`, the
fact tables `cargo xtask check-docs` holds, the README's lane table and counts, and the pinned
counts below. The obligation and synthesis counts in the README are what `cargo xtask check-ess`
reports after regeneration; the spec story records them.

| lane | today | after the stdin spec story | after the live spec story | pinned by |
|---|---|---|---|---|
| portable | 18 | 19 | 20 | `PORTABLE_DECLARED`; `PORTABLE_EXECUTED` 17/18 → 18/19 → 19/20 (on macOS / elsewhere), `ess_confinement_local.rs` |
| delegated | 14 | 17 | 20 | `LINUX_DELEGATED` 9 → 10 → 11, `ess_confinement_linux.rs`; the systemd runner pins no count and requires each inner to run (5 → 7 → 9) |
| macos | 8 | 9 | 10 | `MACOS_DECLARED`, and a new `MACOS_IO` 1 → 2 beside `MACOS_S1` and `MACOS_S2`; the stdin spec story adds it and admits `macos-io-*` in the stage assertion, `ess_confinement_macos.rs` |
| oci | 5 | 6 | 7 | `lane_size` (no constant); the README |
| pod | 3 | 4 | 5 | `lane_size` (no constant); the README |
| interpreted | 1 | 2 | 3 | the README |

**Each spec story and its adapter story land as one unit**, so every lane is green after each pair.
A spec story's commit alone turns the portable, delegated, macos and oci lanes red by construction:
its open steps state a fact no adapter proves yet. That red stays on the unit's branch. The unit
merges into its target only when every lane is green on its head (portable and macos in CI;
delegated and oci as recorded local runs; pod in CI where its lane runs, otherwise recorded), so
neither an integration branch nor `main` carries a red lane between pairs. The `pod` open step does
not change, and the `pod` refusal scenario of each pair passes from its spec commit on.

## 7. The wire

**Nothing reaches the HTTP wire, and no successor bundle is cut.** The adapter family is
substrate-host's API (ADR 0044 decision 7). Design 30 (proposed) puts only `linux` and `macos`
behind the exec and session routes, and there standard input and live output already are the
session family: `/v1/sessions` in `pipes` mode with its frames, bounds and refusals. The exec
route keeps `/dev/null`. `crates/substrate-wire/src/lib.rs` gains no item, and
`cargo xtask check-bundles` and `check-json` see no changed byte. The session codes this design
reuses are existing wire constants, read by the host and published nowhere new.

If design 30 is accepted, its § 4.3 list of published properties should leave `stdin_pipe` and
`output_live` out, because on the wire the session routes already state them (§ 10, Q6).

## 8. Security envelope

- **No new host reach.** Stdin is an anonymous pipe the adapter creates and the child inherits as
  descriptor 0: no path, socket, mount, namespace, capability or seccomp change. Under `linux` it is
  `ChildChannel::Pipes`, which sessions already pass through bubblewrap. Under `oci` the bytes
  travel the runtime connection `start --attach` already holds, and with `--log-driver none` the
  runtime keeps no copy. `pod` gains no Role verb: it serves neither capability.
- **Stdin bounds.** At most 16 MiB in total per run, 64 KiB per write, one write in flight, nothing
  buffered by the adapter. No rate bound: memory does not grow with rate (§ 2). A cancelled write
  closes stdin, a write pending at the observed end fails, and a write after it is refused.
- **Output bounds unchanged.** The same per-stream byte bound, the same 1 s stall deadline, a queue
  of at most 16 chunks per run, and terminal backpressure as ADR 0031 states. Under `Live` the
  record stays inside the same bound and carries no marker.
- **Declared insecure stays declared.** `local` gains a pipe, not confinement. Its admission still
  needs a reason, and every observation still carries it.
- **No silent degradation.** An adapter that cannot serve either capability refuses it by fact
  name at admission, `oci` on an unpinned runtime included. A byte lost at the bound or a stall is
  a named event, a stall stays observable after the events were read, and on any other imposed end
  the record keeps what delivery did not reach.

## 9. Tests and CI cost

Each case runs its scenario once and finishes in seconds. Linux cases lower
`io_bounds.output_bytes` to 4096. Fixed-bound adapters write just past 1 MiB, as design 29's
retention cases already do. Each case goes into its adapter's existing lane test file, and no test
binary is added. Each delegated case prints `<name>: executed` for `require_executed` in
`scripts/delegated-lane.sh`. A case added to `ess_confinement_macos.rs` moves the `15 passed` that
`.github/workflows/macos.yml` pins, in the adapter story that adds it.

Two drivers are shared by the cases below:

- **The fill.** Write 64 KiB chunks to a child that does not read, each write raced against
  100 ms, until one has not returned; at most 16 chunks, or the case fails. A pipe's capacity
  differs by platform (64 KiB on Linux, less on macOS), so the count varies, but a write to a pipe
  nobody reads cannot complete, so the write that has not returned is pending for good.
- **The gate.** The child waits for a marker file the test creates in a directory the child sees:
  `while [ ! -e "$go" ]; do sleep 0.05; done`. The test decides when the child reads or exits, so
  no case depends on a sleep's timing.

| # | case | lane | file |
|---|---|---|---|
| T1a | `check_streams`: `limit_bytes` 0 and 16 MiB + 1 are `session.limit-unserved`; each writes one `profile_refused` record | portable | `confinement/mod.rs` unit tests |
| T1b | `check_streams`: `queued_chunks` 0 and 17 are `session.limit-unserved`; `Live` with `Tail` is `confinement.live-output-retention-invalid`; each writes one `profile_refused` record | portable | `confinement/mod.rs` unit tests |
| T2 | `local`: `cat` with `Piped` echoes three writes on `Stdout`; `close_stdin` ends it with exit 0; a second `close_stdin` is `Ok`; a write after close is `session.input-closed` | portable | `crates/substrate-host/tests/ess_confinement_local.rs` |
| T3 | `local`: a write past `limit_bytes` is `session.input-limit`, a 64 KiB + 1 write `session.frame-limit`, a write on a `Null` run `session.not-pipe` | portable | the same |
| T4 | `local`, `Live`, natural end: a program writing to both streams and exiting yields contiguous offsets per stream, each chunk equal to the record at its offset, the record equal to the delivered bytes, then `None` | portable | the same |
| T5 | `local`, `Live`, `queued_chunks: 1`, a child writing without end and never read: the run ends no sooner than 1 s and within 3 s of its start (the 1 s deadline, the 1 s group settle, 1 s slack); the queued chunk is readable, then `Stalled`, then `None`; `output_stalled` is true; the record begins with the delivered chunk | portable | the same |
| T6 | `local`, `Live`, natural end past the bound: `Bounded { Stdout }` after the last chunk, then `None`; `stdout_truncated`; the delivered stdout offset equals the bound; the record equals the delivered bytes, with no marker | portable | the same |
| T7a | `run_with_stdin`: `cat` returns the input on stdout; a child that exits without reading returns accepted bytes ≤ input and no error; a `Null` profile is `session.not-pipe` and an input longer than `limit_bytes` `session.input-limit`, each with no start record; a gated child that never reads writes its pid to a file, the helper, given 1 MiB of input, is dropped 200 ms after the pid file appears, and that process is gone within 3 s | portable | the same |
| T7b | `run_with_stdin` with a `Live` profile is `confinement.live-output-unread`, with no start record | portable | the same |
| T8 | `local`, cancel: a gated child that then runs `wc -c`; the fill; the pending write is cancelled. The next write is `session.input-closed` and `close_stdin` is `Ok`. The test opens the gate: the child reports at least the bytes of the completed writes and at most 65 536 more, and exits 0, so it read end of file | portable | the same |
| T9 | `local`, close during a pending write: with T8's pending write, a concurrent `close_stdin` has not returned after 200 ms; once the write is cancelled it returns `Ok` | portable | the same |
| T10 | `local`, the run's end: (a) `true`; after `wait` returned, a write is `session.input-closed` and `close_stdin` is `Ok`. (b) a gated child that never reads; the fill; the test opens the gate, the child exits, and the pending write returns `session.write-failed`. (c) as (b), but the child first starts a descendant that leaves the process group and holds only descriptor 0: `exec 3<&0; perl -MPOSIX -e 'setsid; exec @ARGV' sleep 30 <&3 >/dev/null 2>&1 3<&- & echo $! > "$pid"` (descriptor 3 because a non-interactive shell gives a background command `/dev/null`; output redirected so the `local` run can end; `perl` because macOS has no `setsid` utility). After the gate opens, `wait` returns and the pending write returns `session.write-failed` within 1 s of it; the test then kills the descendant by its pid | portable | the same |
| T11 | `local`, `Live`, `queued_chunks: 2`, both streams writing and never read: one `Stalled`, after the last chunk of either stream; each stream's chunks contiguous | portable | the same |
| T12 | `linux`: T2, T4, T5, T6, T8, T9, T10 (a) and (b), T11, T19, and `Piped` + `Recorded` with 64 KiB of unread output not stalling | delegated | `ess_confinement_linux.rs` |
| T13 | `systemd` around `local` and around `linux`: T2, T4 and T5 once each around each inner (T5 around `linux` reads `output_stalled` through the wrapper), so every forwarded method answers | delegated | `ess_confinement_systemd.rs` |
| T14 | `macos`: T2 and T4 | macos | `ess_confinement_macos.rs` |
| T15 | `oci`, on the lane's pinned runtime: `cat` in the container, three writes with the bytes `0x10 0x11` among them, then `close_stdin`; the run exits 0 and its stdout equals the bytes written, so every byte and the end of file crossed the runtime client; and T4 | oci | `ess_confinement_oci.rs` |
| T16 | `oci`: a probe naming a runtime version outside `OCI_STDIN_PINNED` proves no `stdin_pipe`, and `Piped` is `confinement.fact-unserved` naming `stdin_pipe` | portable | `oci.rs` unit test |
| T17 | `pod` refuses `Piped` naming `stdin_pipe` and `Live` naming `output_live`, `confinement.fact-unserved`, through `check_profile` | portable | `pod.rs` unit test |
| T18 | the daemon's `PIPE_MAX_INPUT_BYTES` equals `confinement::MAX_STDIN_BYTES` | portable | `crates/substrate-daemon/src/app.rs` unit test |
| T19 | `local`, `Live`, imposed end: a child writing without end, read twice, then `signal(Kill)`: the remaining queued chunks, then `None`, no `Stalled`; `output_stalled` is false; the delivered chunks are a prefix of the record | portable | `ess_confinement_local.rs` |

T4 and T6 assert that the record equals the delivered bytes only on a natural end; T5, T11 and
T19 assert the prefix. The scenarios of § 6 run in the lanes that already exist, so no CI job is
added. A stall case costs one stall deadline (1 s) plus the kill; T5, T11 and their `linux` and
`systemd` repeats are the only ones. The default profile changes no existing case.

`tests/confinement-public-items.txt` gains only these lines: `StdinMode`, `OutputDelivery`,
`OutputEvent`, `PipeStream` re-exported, the two profile fields, the four trait methods,
`run_with_stdin`, the two `Fact` variants, `FACT_ORDER`'s new length, `MAX_STDIN_BYTES`,
`STDIN_CHUNK_BYTES`, `MAX_QUEUED_CHUNKS`, `LIVE_OUTPUT_RETENTION_INVALID` and `LIVE_OUTPUT_UNREAD`.
The `Fact` and `FACT_ORDER` lines move with each spec story, the others with each adapter story.
As documentation, the table in `profile.rs` changes, along with the field table in
`website/docs/guides/confinement-profiles.md`. The fact tables in
`website/docs/concepts/adapters.md` and `website/docs/reference/contracts.md` move with the spec,
as `cargo xtask check-docs` requires.

## 10. Decisions

| id | decision | choice | rejected |
|---|---|---|---|
| D1 | stdin surface | profile `StdinMode::Piped { limit_bytes }`, `write_stdin` and `close_stdin`, and the helper `run_with_stdin` | the whole buffer in `Command` (breaks `Command`, no long-lived child); the raw `ChildStdin` (not every adapter holds it) |
| D2 | stdin bounds and pending writes | total 1 to 16 MiB, write 1 to 64 KiB, no adapter buffer, ended by EOF; one shared `StdinWriter`; a cancelled write poisons and closes stdin, under `linux` through `Handle::try_current` with the next call as the fallback; `close_stdin` waits for a pending write | unbounded; a bytes-per-second rate bound (memory does not grow with rate); a write deadline inside the adapter (cancellation already bounds it); `tokio::spawn` in `Drop` (panics without a runtime) |
| D3 | live output surface | `OutputDelivery::Live { queued_chunks }`, `read_output(run, timeout)` returning `Chunk`, `Bounded` or `Stalled` under a per-run reader lock; `Bounded` from the delivered offset and the final `*_truncated`, `Stalled` from the stall flag, both after the queue; `output_stalled(run) -> bool` | polling `output()`; an unbounded channel; a lossy live view; loss events in the queue (no slot when it is full); per-stream drain bound flags (the host has none) |
| D4 | backpressure | terminal after the 1 s stall deadline, ADR 0031 unchanged, extended from daemon sessions to in-process embedders | blocking the child; dropping chunks; a configurable deadline |
| D5 | live output and the record | the record begins with the delivered head and equals it on a natural end; no marker under `Live`; `Live` with a non-`Head` retention refused (422) | a record with its own retention beside the live stream |
| D6 | ordering and loss | per stream in order, contiguous offsets; at a natural end every byte within the bound is delivered and the bound is named; no order between streams; the contract counts chunks, not bytes | a total order across stdout and stderr (not observable from two pipes); coalescing to 64 KiB chunks (holds back written lines) |
| D7 | capability statement | two facts, `stdin_pipe` and `output_live`, appended to `Fact` | two new families (changes atlas ADR 0109 decision 8); host-local refusals with no fact (no selection by facts) |
| D8 | refusal names | session codes of `substrate_wire` for per-call refusals; two new codes, `confinement.live-output-retention-invalid` and `confinement.live-output-unread`, on the existing `Driver` variant | a parallel `confinement.*` vocabulary for the same refusals; a new `ConfinementError` variant (a further break) |
| D9 | delivery | a spec story per fact; each spec story with its adapter story as one unit, merged only with every lane green on its head; stdin first, live output after it; `local`, `linux`, `systemd`, `macos`, `oci` | one spec story for both facts (no adapter unit could show a green lane, review pass 2 G5); one wave for all of it; adapter stories that edit the suite (ADR 0044 decision 6) |
| D10 | `pod` | neither capability: stdin needs new Role authority, and live output waits for a merged-stream statement | `pods/attach` now; live output labelled `Stdout` |
| D11 | wire | no change, no bundle | a successor bundle for adapter stdin (the session family already serves it on the wire) |
| D12 | `run_with_stdin` | refuses `Null`, an input over `limit_bytes` and (with `output_live`) `Live` before start; a dropped helper kills its run | draining and discarding the events inside the helper (hides the stream the profile asked for); leaving a cancelled helper's run running |
| D13 | `oci` `stdin_pipe` | declared only for runtime and version pairs the oci lane pinned; refused by fact name otherwise | a static declaration for every runtime (a detach sequence could eat bytes on an unlisted one); a probe at open (a later option) |
| D14 | public API | three stated breaks (`Fact`, `FACT_ORDER`, `ConfinementProfile`), new enums `#[non_exhaustive]` | marking `ConfinementProfile` `#[non_exhaustive]` now (breaks every outside literal) |
| D15 | stdin at the run's end | from the adapter's observed end, a write is `session.input-closed` and close `Ok`; a write pending at that moment is `session.write-failed`; the adapter closes its pipe then | leaving the host's stdin open until the exec is forgotten (a late write would succeed into a dead run's pipe) |
| D16 | live delivery on an imposed end | delivery stops; queued chunks stay readable; the record is the complete account; the stall alone is a named event | an event per imposed end (needs a cause `RunObservation` does not carry today); forwarding until the drain ends (the host closes the receiver first) |

Open questions for the operator, each with a recommended default:

| id | question | recommended default |
|---|---|---|
| Q1 | Fact names | `stdin_pipe` and `output_live` |
| Q2 | Allow `Live` with `Tail` or `HeadAndTail` later? | refuse now. Lifting it needs a record kept apart from the delivered head, and a consumer that asks |
| Q3 | `pod` stdin through `pods/attach` | defer to its own design, written when a consumer needs it, with the Role change and the WebSocket attach client as its subject |
| Q4 | `oci` stdin on a runtime outside the pinned list | decided by D13: refused by fact name. A probe at open can replace the list later |
| Q5 | Does acceptance need an ADR? | no. ADR 0044 decision 1 has adapters declare facts, ADR 0007 fixes the stdin and close semantics, and ADR 0031 (which superseded ADR 0016) fixes the backpressure this design reuses. ADR 0031 was decided for daemon sessions; applying its unchanged deadline and kill to an in-process embedder is recorded here (D4), not in a successor ADR, because neither the rule nor a bound changes. Atlas learns of the two facts from the CHANGELOG, and no atlas file changes |
| Q6 | Should design 30 say it does not publish the two facts? | yes, one sentence in design 30 § 4.3 when it is accepted; not edited here |
| Q7 | A stall visible on `RunObservation` as well as the `Stalled` event | decided by D3: `output_stalled(run)` on the adapter, a non-breaking trait method. A `RunObservation` member stays out: a further public-struct break |
| Q8 | `pod` live output | defer until a design states merged-stream delivery; written when a consumer needs it |

## 11. Stories

Four stories in two pairs, in this order. Each pair is one unit on its target branch: the spec
story's commit, then the adapter story's, merged only when every lane is green on the unit's head
(§ 6). The second pair starts from the first pair's merged head. No single wave is needed for all
four.

1. `story:adapter-io-spec`, narrowed to standard input: § 6's stdin half. The fact `stdin_pipe`,
   the outcome `fact-unserved-stdin-pipe`, the declared sets, every open-step edit, the eight
   stdin scenarios, the regenerated suite and `Fact`, the `FACT_ORDER` length, the public-items
   and fact-table lines, the README counts and the pinned lane counts (the `MACOS_IO` pin and the
   macos stage assertion included). No adapter code, no adapter proof.
2. `story:adapter-stdin-pipe`, `depends_on` story 1: `StdinMode`, the `StdinWriter`, `write_stdin`,
   `close_stdin`, the run's-end rule, `run_with_stdin` without its `Live` refusal, `check_streams`
   with the `limit_bytes` check, `stdin_pipe` on every adapter of § 5 that proves it,
   `OCI_STDIN_PINNED`, `systemd` around `linux` forwarding `write_stdin` and `close_stdin`, and the
   `linux` `Pipes` channel without a live sender (T1a, T2, T3, T7a, T8 to T10, T16, T18, and the
   stdin rows of T12 to T15 and T17). Edits no scenario and no pinned scenario count.
3. `story:adapter-live-spec`, `depends_on` story 2: § 6's live half, as story 1 for `output_live`.
4. `story:adapter-live-output`, `depends_on` stories 2 and 3: `OutputDelivery`, `OutputEvent`, the
   live queue in the shared drain and in `drain_capped`'s adapter path, the `linux` `Pipes` channel
   with a live sender, `read_output` and its reader lock, `output_stalled`, the stall accessor,
   `systemd` around `linux` forwarding both, `check_streams`'s `queued_chunks` and retention
   checks, `run_with_stdin`'s `confinement.live-output-unread` refusal and `output_live` (T1b, T4
   to T6, T7b, T11, T19, and the live rows of T12 to T15 and T17). Edits no scenario and no pinned
   scenario count.

Story 4 builds on story 2's `check_streams`, `run_with_stdin`, linux `Pipes` channel and `systemd`
forwarding, and both edit `confinement/mod.rs`, `local.rs`, `systemd.rs`, `linux.rs`, `macos.rs`,
`oci.rs` and `process.rs`, hence its edge on story 2. The consumer's move from `HostDriver` sessions
to the adapter is its own change after its pin moves, and none of it is done here.

## 12. Out of scope

- A pty on any adapter, and unrecorded capture ([design 23](23-terminal-capture-selection.md)) on
  any adapter.
- `pod` standard input and `pod` live output (D10, Q3, Q8).
- A probe at open for `oci` standard input (D13).
- Naming on `RunObservation` which bound imposed a run's end (D16).
- Any daemon route, wire type or bundle.
- A `b10x-substrate confine run` flag that forwards its own stdin. That can follow as its own
  change.
- Reconnecting to a run after the embedding process died. Runs remain in-process, as today.

## 13. Review passes

Pass 1 reviewed the first draft and returned needs-revision with eight blocking findings and five
notes (F1 to F13). Pass 2 reviewed the revision, held F4 to F7 and F9 to F13, reopened F1, F2, F3,
F8 and F12, and returned needs-revision with five blocking findings and four notes (G1 to G9).
Where each is answered:

| finding | answer |
|---|---|
| F1 the record is not what was delivered (marker; filling after a stall) | § 1 rows on `drain_capped`; § 3 *The record holds the delivered head*; § 5 `linux` row (`mark_truncation = false`); D5; T4, T6. Reopened as G1 |
| F2 no mechanism for loss events | § 3 *Loss events are synthesized after the queue*; § 5 `local` and `linux` rows; D3; `Interrupted` removed with `pod` live output. Reopened as G2 |
| F3 cancellation has no mechanism | § 2 *One writer per run* and *Cancellation closes*; D2; T8. Reopened as G3 |
| F4 pending writes | § 2 *Pending writes* and *The run's end*; D2, D15; T9, T10 |
| F5 `run_with_stdin` under `Live` stalls | § 2 `run_with_stdin` (refused `confinement.live-output-unread`; full duplex needs a concurrent reader); D12; T7b |
| F6 static `oci` fact for a lane result | § 5 `oci` row (`OCI_STDIN_PINNED`); D13; Q4; T15, T16 |
| F7 public API breaks understated | § 1 public-items row; § 4 *Public API compatibility*; D14; § 9 public items |
| F8 stories cannot land independently | § 6 per-fact spec stories and units; D9; § 11 edges. Reopened as G5 and G6 |
| F9 stall threshold and memory | § 3 *Chunk sizes in this tree* and *Unread output*; D6 |
| F10 Q7 fails in the stall case | `output_stalled` in § 3; D3; Q7; T5 |
| F11 `pod` merged stream labelled `Stdout` | § 5 `pod` row: `pod` live output deferred; D10; Q8; T17 |
| F12 missing cases; T5's time bound | T5 (within 3 s), T8 cancel, T9 close during a pending write, T10 child exit during a write, T11 stall with both streams, T13 stall on `systemd`. Reopened as G7 |
| F13 ADR 0031 extended to non-session embedders | § 3 *Backpressure is terminal*; D4; Q5 |
| G1 any imposed end on `linux` closes the receiver with no event | § 1 row on imposed ends; § 3 *Delivery stops when the run is ended for it*, *Ordering and loss*, *The record holds the delivered head*; D16; T4 and T6 hold equality to natural ends; T19 |
| G2 no per-stream bound flags | § 1 row on `run_child`; § 3 *Loss events*: `Bounded` from the delivered offset and the final `*_truncated`; § 5 `linux` row; D3; T6 |
| G3 `tokio::spawn` in `Drop`; concurrent `read_output` | § 2 *Cancellation closes* (`Handle::try_current`, fallback stated); § 3 *One reader at a time*; D2, D3 |
| G4 write after the run's end undefined; T10 races | § 1 row on the host's stdin; § 2 *The run's end* (`session.input-closed` from the observed end); D15; T10 (a) to (c) driven by `wait` and the gate |
| G5 one spec story keeps every adapter unit red | § 1 row on lane pins; § 6 per-fact spec stories, the pinned-count table, units green per pair; D9; § 11 |
| G6 story 2 reads story 3's types; no edge | § 2 `run_with_stdin` (the `Live` refusal arrives with `output_live`); § 4 `check_streams` split; § 11 story 4 `depends_on` story 2, `confinement.live-output-unread` and T7b in story 4; T1a and T1b |
| G7 T8 and T9 never cancel; T10's descendant keeps `local` running | § 9 *the fill* and *the gate*; T8, T9; T10 (c) redirects the descendant's output and takes descriptor 0 explicitly |
| G8 `run_with_stdin` can leave a started run running | § 2 `run_with_stdin` (refusals before start; a dropped helper kills its run); signature takes the adapter `Arc`; D12; T7a |
| G9 `oci` pipe meaning; T15 EOF; `systemd` forwarding; ADR 0016 cited as current | § 2 *No buffering*; § 5 `oci` and `systemd` around `linux` rows; T13, T15; ADR 0031 cited throughout as ADR 0016's successor |
