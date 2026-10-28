# Design 32: standard input and live output on the confinement adapters

Status: proposed, 2026-10-28, revised after review pass 1 the same day (§ 13). § 11 names the
stories. It changes no wire byte and no contract bundle: the adapters are substrate-host's
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

| what | where |
|---|---|
| every adapter spawns with `stdin(Stdio::null())`: `local`, `macos`, `systemd` around `local`, and the `oci` runtime client (`start --attach`) | `local.rs` `start_run`, `macos.rs` (the confined command), `systemd.rs` (the run's spawn), `oci.rs` `start_checked` |
| `linux` starts through `HostDriver::start_exec_for_adapter`, which reaches `start_inner` with no pipe settings, so the child channel is `ChildChannel::Quiet` (stdin `/dev/null`, output captured) | `crates/substrate-host/src/lib.rs`, `crates/substrate-host/src/process.rs` |
| `pod` renders `"stdin": false, "tty": false` and reads the log once the container terminated, `limitBytes` = bound + 1. The kubelet's log merges stdout and stderr into one stream (design 28) | `pod.rs` (the container spec, `read_log`) |
| `local`, `macos`, `systemd` around `local` and `oci` drain both pipes into one shared `Captured` through `output_retention::drain`, reading 16 KiB at a time; `output()` while running returns a snapshot. `linux` reports output only once the run ended | `crates/substrate-host/src/output_retention.rs`; design 29 § 6 |
| the host already serves live stdin and output for sessions: `ChildChannel::Pipes`, `write_pipe` (frame ≤ `frame_limit`, total ≤ `input_limit`, refusals `session.frame-limit`, `session.input-limit`, `session.input-closed`, `session.write-failed`), `close_pipe_input` (half-close, idempotent), `read_pipe` (`session.read-timeout`, `session.timeout-invalid`) | `process.rs` |
| `write_pipe` holds the stdin mutex across `write_all` and `flush`, and `close_pipe_input` takes the same mutex, so a close waits behind a write the child does not read. A `write_pipe` future dropped mid-frame may have written part of it, and nothing records that | `process.rs` `write_pipe`, `close_pipe_input` |
| live output is a bounded queue of `PipeFrame { stream, bytes }` and nothing else: frames ≤ `PIPE_FRAME_BYTES` (64 KiB), at most `PIPE_QUEUED_FRAMES` (16); a frame not reserved within `PIPE_OUTPUT_STALL_TIMEOUT` (1 s) sets the execution's `pipe_backpressure` flag, records `session.output-backpressure` and ends the tree; the drain continues so the child never blocks; forwarding stops at the stream's output bound, which sets that stream's `bound_reached` flag | `observation.rs` `PipeFrame`; `process.rs` `drain_capped`, `forward_frame`; [ADR 0016](../../adr/0016-pipe-output-backpressure-is-terminal.md), [ADR 0031](../../adr/0031-live-output-backpressure-has-a-stall-deadline.md) |
| `drain_capped` reads 8 KiB at a time and forwards each read as one frame, so a frame is at most 8 KiB in practice | `process.rs` `drain_capped` |
| with a live sender, `drain_capped` keeps `Head`, but the record is not the delivered bytes in two cases: a raw-pipe drain passes `mark_truncation = true`, so a truncated record's last 31 bytes are replaced by `TRUNCATION_MARKER`; and after a stall it keeps filling the record up to the bound while forwarding has stopped. The pty path passes `mark_truncation = false` | `process.rs` `drain_capped` and its three callers |
| the daemon caps a session's total input at `PIPE_MAX_INPUT_BYTES` (16 MiB) | `crates/substrate-daemon/src/app.rs` |
| every adapter tracks at most 128 runs | `LOCAL_MAX_TRACKED_RUNS`, `SYSTEMD_MAX_TRACKED_RUNS`, `POD_MAX_TRACKED_RUNS`, `HostConfig::max_tracked_execs` |
| `oci`'s open reads the runtime's flavor and version (`RuntimeProbe`) | `oci.rs` `parse_info` |
| public and not `#[non_exhaustive]`: `ConfinementProfile` (public fields, `Default`), `ConfinementError`, `RunObservation`, and the generated `Fact` (serde `Serialize` and `Deserialize`); `FACT_ORDER: [Fact; 11]` and `FAMILY_ORDER: [ContractFamily; 10]` are public fixed-length arrays | `confinement/mod.rs`, `confinement/profile.rs`, `confinement/facts.rs`, `crates/substrate-host/src/generated/confinement.rs`, `crates/substrate-host/tests/confinement-public-items.txt` |

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
pub async fn run_with_stdin(
    adapter: &dyn ConfinementAdapter,
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
  (422). A run that ends closes it.
- **No buffering.** A write goes straight to the pipe and returns once the pipe took every byte.
  At most one write is in flight per run: a second `write_stdin` on the same run waits for the
  first. A child that does not read blocks the writer, not the host. A failed write (the child
  closed its stdin or ended) is `session.write-failed` (500) and never reported as delivered.
- **One writer per run, shared by every adapter.** A `pub(crate)` adapter-side `StdinWriter` in
  `confinement/mod.rs` holds, per run, a `tokio::sync::Mutex` that every write and close takes, and
  a `poisoned: AtomicBool`. A write takes the lock, answers `session.input-closed` if `poisoned` is
  set, arms a drop guard, writes the chunk through the adapter's pipe, and disarms the guard. Under
  `linux` the pipe is the host driver's `write_pipe` and `close_pipe_input` by exec id; elsewhere it
  is the `ChildStdin` of the process the adapter spawned (the child, or the `oci` runtime client).
- **Cancellation closes.** A `write_stdin` future dropped while its guard is armed may have
  written part of its chunk. The guard's `Drop` sets `poisoned` synchronously and closes the pipe
  where that is synchronous: it owns the lock guard over the `ChildStdin` and drops it (`local`,
  `systemd` around `local`, `macos`, `oci`). Under `linux` it spawns `close_pipe_input` on the
  current runtime, and the next write or close, holding the lock and seeing `poisoned`, calls it
  again first (it is idempotent). After a cancellation every write is `session.input-closed`, and
  `close_stdin` returns `Ok`. The child reads the part of the chunk the pipe took and then end of
  file: a prefix, never a splice.
- **Pending writes.** `write_stdin` has no deadline of its own: the embedder bounds it by
  cancelling it, which closes stdin as above. `close_stdin` takes the same lock, so it waits while
  a write is pending; an embedder that wants to close now cancels the pending write, after which
  `close_stdin` returns `Ok` at once. A write pending when the run ends returns
  `session.write-failed`: the writer races each chunk against the run's end, and when the end wins
  it drops the pipe. This matters where the adapter does not prove `whole_tree_kill` (`local`,
  `macos`): a descendant that left the run's process group can still hold descriptor 0 after the
  run ended, and without the race it would hold the writer forever. Where the run is a cgroup, its
  end means every holder is gone and the write fails on its own.
- **No rate bound beyond the pipe.** The writer is the embedder, which already chooses the argv;
  host memory is bounded by the one chunk in flight, and the child's read rate paces the writer.
  The reserved `rate_bounds` field (ADR 0044 decision 5) stays reserved.
- **Wrong run.** `write_stdin` or `close_stdin` on a run admitted with `Null` is `session.not-pipe`
  (422). An unknown run is the adapters' existing not-found.
- **`run_with_stdin`** refuses a profile admitted with `OutputDelivery::Live` before it starts
  anything: `confinement.live-output-unread` (422). It reads no events, so a `Live` run would stall
  once its queue filled and be killed after the 1 s deadline. Under `Recorded` it writes in 64 KiB
  chunks while it waits, and stops writing without failing when the child closed its stdin or ended
  first (`session.write-failed`): the child read what it read. It returns the number of bytes the
  pipe accepted. Under `Recorded` the adapter drains output on its own, so writing all input before
  reading output cannot deadlock. Full duplex under `Live` needs a concurrent reader: the embedder
  calls `write_stdin` and `read_output` from two tasks, which the trait allows, since both take
  `&self` and may be in flight on one run at once.
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
| **B** | **a bounded per-run queue of chunks, read with `read_output(run, timeout)`, terminal backpressure after the stall deadline (ADRs 0016 and 0031)** | the session contract, already decided and implemented in `drain_capped`; ordered and lossless per stream |
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
- **Loss events are synthesized after the queue, not queued.** The host queue carries only chunks,
  and a stall happens exactly when it is full, so a queued `Stalled` would have no slot.
  `read_output` therefore returns the queued chunks in order and counts each stream's `offset`
  itself. Once the queue is exhausted (every drain ended and every chunk was read) the drain's flags
  are final, and it returns, in this order and each at most once: `Bounded { Stdout }` if stdout
  crossed its bound and its delivered bytes reached the bound; `Bounded { Stderr }` likewise;
  `Stalled` if the stall flag is set; then `None` from then on. Under `linux` the flags are the
  execution's `pipe_backpressure` and the two per-stream bound flags `drain_capped` already sets,
  read through a new `pub(crate)` `HostDriver` accessor by exec id. Under the shared drain they are
  the same three flags, kept in the adapter's run state.
- **Ordering and loss.** Within one stream: in order, contiguous (`offset` + `bytes.len()` is the
  next chunk's `offset`), no duplicate, no gap. Between stdout and stderr: no order is promised.
  A pipe pair has none to observe. A loss is always a named event (`Bounded`, `Stalled`), after
  every chunk and before `None`, never in place of a chunk.
- **The record holds the delivered head.** With `Live`, `output()` after the run holds each
  stream's head within the bound, and for every chunk `stdout[offset..offset + len] == bytes` (and
  likewise for stderr): each stream's record begins with exactly the bytes delivered. It equals them
  unless the run stalled. After a stall the drain keeps filling the record up to the bound while
  delivery has stopped, so the record can hold bytes past the last chunk, and `output_stalled` says
  so. No truncation marker is written into a `Live` record: under `linux` the adapter's live drain
  passes `mark_truncation = false`, as the pty path does, so the marker never replaces the last
  31 bytes of a delivered head; `stdout_truncated` and `KeptOutput` say what is missing.
  `KeptOutput` reports `Head`. A profile asking `Live` together with `Tail` or `HeadAndTail` is
  refused `confinement.live-output-retention-invalid` (422), because the record could then not
  begin with the delivered bytes (§ 10, Q2). A `Recorded` record is unchanged, the marker included.
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
appended to `Fact` after `image_digest_pinned`, so `FACT_ORDER` names every existing fact first and
no existing refusal changes. A profile requiring one the adapter did not prove is
`confinement.fact-unserved` (501) naming it, at admission, with its `profile_refused` audit record.
The admission record's `facts` lists them, so the audit says the run had a pipe. The stdin bytes
and output chunks are never audited or logged.

The range checks (`limit_bytes`, `queued_chunks`) and the retention conflict sit in one shared
`pub(crate) fn check_streams(&ConfinementProfile)` every adapter's `check` calls first, as
`check_workspace_access` is shared, so a caller's mistake gets the same answer on every adapter.

### Public API compatibility

Three breaks, each one `CHANGELOG.md` line under the next minor. Every consumer pins an exact
revision and the crate is 0.x, so none is hidden behind a version a consumer did not choose:

1. **`Fact` gains `StdinPipe` and `OutputLive`.** It is generated from the ESS domain, is not
   `#[non_exhaustive]` and derives `Serialize` and `Deserialize`. An exhaustive `match` breaks, and a
   consumer on an older revision fails to deserialize an `AdapterFacts` or an audit record that
   names either fact. It stays exhaustive: it is generated, and changing the generator is not this
   design.
2. **`FACT_ORDER` changes type** from `[Fact; 11]` to `[Fact; 13]`: code that names the array
   type or relies on its length breaks. `FAMILY_ORDER` (`[ContractFamily; 10]`) does not change,
   because no family is added.
3. **`ConfinementProfile` gains `stdin` and `output_delivery`.** It is not `#[non_exhaustive]`, so
   a struct literal without `..ConfinementProfile::default()` and an exhaustive destructuring
   pattern break. Marking it `#[non_exhaustive]` now would break every literal outside the crate,
   so it is not done here.

Not breaks: the four trait methods carry default bodies, so an implementor outside the crate
compiles unchanged. `StdinMode` and `OutputDelivery` are `#[non_exhaustive]` from the start, as
`OutputEvent` is, so a later mode or event is additive. `ConfinementError`, `RunObservation`,
`RunOutput`, `Command` and `AdapterFacts` keep their shape.

## 5. Per-adapter feasibility

| adapter | `stdin_pipe` | `output_live` | mechanism |
|---|---|---|---|
| `local` | proved | proved | `Stdio::piped()` for stdin, held by the shared `StdinWriter`; `output_retention::drain` gains an optional live queue that it fills after the record, with the stall and bound flags in the run state; the stall kills the process group |
| `systemd` around `local` | proved | proved | its own spawn, the same code as `local`; the stall kills the run's cgroup |
| `linux` | proved with the floor | proved with the floor | the host driver's session path. `AdapterStart` gains `stdin: bool` and `live: Option<queue>` (`pub(crate)`). `ChildChannel::Pipes` without a live sender is new: stdin piped, output recorded through `drain_capped` with no sender, so `Piped` + `Recorded` never stalls. With a live sender the adapter's drain passes `mark_truncation = false`. `write_pipe`, `close_pipe_input` and `read_pipe` serve the adapter by exec id behind the `StdinWriter`, and a `pub(crate)` accessor reads the three flags. Sessions keep their path unchanged |
| `systemd` around `linux` | as `linux` | as `linux` | delegates to `linux` |
| `macos` | proved once the macos lane case passes | the same | the same `Captured` path as `local`. Seatbelt rules govern opens by path, and the run inherits descriptors 0 to 2. The macos lane proves that a confined `cat` reads and writes them before the fact is declared (ADR 0023's rule: nothing published unproved) |
| `oci` | proved only for pinned runtime and version pairs | proved | `create --interactive` and `start --attach --interactive` with the runtime client's stdin piped; output is already the attached client's two pipes. The runtime's detach sequence (`ctrl-p ctrl-q`) must not eat input bytes: the adapter disables it where the runtime has an option for it. `open` declares `stdin_pipe` only when the probe's flavor and version are a pair in `OCI_STDIN_PINNED`, a list that holds only pairs on which the oci lane wrote the bytes `0x10 0x11` through `cat` and read them back. On any other pair the fact is absent and `Piped` is refused `confinement.fact-unserved` naming `stdin_pipe` (invariant 3; atlas ADR 0109 decision 2). A probe at open, writing those bytes through a throwaway container, is a later option and not this design |
| `pod` | **not served** | **not served** | stdin needs `stdin: true` and `stdinOnce: true` in the Pod and the `pods/attach` subresource on the Role: new authority (attach to any Pod in the namespace) and a new streaming client. Live output is deferred: the log endpoint merges stdout and stderr into one stream, so a chunk could name no true `PipeStream`, and labelling the merged stream `Stdout` would misstate it. It waits for a design that states merged-stream delivery (an event or fact that says the streams are one), written when a consumer needs it. Both refused `confinement.fact-unserved` |

One wave serves `local`, `linux`, `systemd` (both inners), `macos` and `oci` (§ 6, § 11). Within
it, the `local`, `linux` and `systemd` parts come first: they are what the consumer runs on.

## 6. The ESS confinement domain and suite

Changes in `spec/confinement/confinement.yaml`:

- `Fact` gains `stdin_pipe` and `output_live`, with a comment line each, after `image_digest_pinned`.
- `AdmitProfile` gains the outcomes `fact-unserved-stdin-pipe` and `fact-unserved-output-live`,
  after `fact-unserved-image-digest-pinned`.
- The declared sets above `OpenAdapter` gain the facts as § 5 states, each with its source. The
  `oci` set states the facts the oci lane's pinned runtime yields. The `pod` set does not change.

The model does not see `limit_bytes`, `queued_chunks` or the retention conflict: like design 29's
refusals, they are raised by an adapter's `check`, which `AdmitProfile` does not model, and are held
by the Rust cases of § 9. Design 29 § 10 records that gap, and this design does not close it.

New authored scenarios, named by the lane rule of `spec/confinement/README.md`:

| scenario | lane |
|---|---|
| `local-admits-stdin-pipe-and-output-live-on-portable` | portable |
| `linux-admits-stdin-pipe-and-output-live-on-delegated` | delegated |
| `systemd-local-admits-stdin-pipe-and-output-live-on-delegated` | delegated |
| `systemd-linux-admits-stdin-pipe-and-output-live-on-delegated` | delegated |
| `model-stdin-pipe-and-output-live-unserved-on-interpreted` (an adapter set without either fact, refused by each name) | interpreted |
| `macos-admits-stdin-pipe-and-output-live-on-macos` | macos |
| `oci-admits-stdin-pipe-and-output-live-on-oci` | oci |
| `pod-stdin-pipe-unserved-on-pod` | pod |
| `pod-output-live-unserved-on-pod` | pod |

**One spec story, first.** Every lane compares the adapter's real facts with the scenario's open
step exactly (`crates/substrate-host/tests/support/mod.rs`), so adding a fact to an adapter also
edits the open step of every authored scenario that opens it (at the time of writing 11 files name
`local`, 11 `linux` and 7 `systemd`, besides every `macos` and `oci` one). ADR 0044 decision 6 keeps
adapter stories out of the suite, so one spec story makes every spec change of this design: both
facts, both outcomes, every open-step edit and every scenario in the table. Both adapter stories
depend on it and neither edits the suite.

It lands first, on the wave's integration branch. From that commit until both adapter stories have
landed there, the portable, delegated, macos and oci lanes of that branch are red by construction:
their open steps state facts the adapters do not prove yet. The integration branch merges into
`main` only with every lane green on its head (portable and macos in CI; delegated, oci and pod as
recorded local runs), so `main` never carries a red lane. Splitting the scenarios per fact would not
shorten that window: an open step states the whole declared set, so every scenario that opens
`local` fails until both facts are proved. The `pod` open step does not change, and its two refusal
scenarios pass from the spec commit on. `cargo xtask check-ess` regenerates
`spec/confinement-suite.json` and `Fact`. The README's counts (lanes, synthesized and refused
obligations) move with it.

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
  closes stdin, and a write cannot outlive its run.
- **Output bounds unchanged.** The same per-stream byte bound, the same 1 s stall deadline, a queue
  of at most 16 chunks per run, and terminal backpressure as ADRs 0016 and 0031 state. Under `Live`
  the record stays inside the same bound and carries no marker.
- **Declared insecure stays declared.** `local` gains a pipe, not confinement. Its admission still
  needs a reason, and every observation still carries it.
- **No silent degradation.** An adapter that cannot serve either capability refuses it by fact
  name at admission, `oci` on an unpinned runtime included. A lost byte is a named event, and a stall
  stays observable after the events were read.

## 9. Tests and CI cost

Each case runs its scenario once and finishes in seconds. Linux cases lower
`io_bounds.output_bytes` to 4096. Fixed-bound adapters write just past 1 MiB, as design 29's
retention cases already do. Each case goes into its adapter's existing lane test file, and no test
binary is added. Each delegated case prints `<name>: executed` for `require_executed` in
`scripts/delegated-lane.sh`.

| # | case | lane | file |
|---|---|---|---|
| T1 | `check_streams`: `limit_bytes` 0 and 16 MiB + 1, `queued_chunks` 0 and 17 are `session.limit-unserved`; `Live` with `Tail` is `confinement.live-output-retention-invalid`; each writes one `profile_refused` record | portable | `confinement/mod.rs` unit tests |
| T2 | `local`: `cat` with `Piped` echoes three writes on `Stdout`; `close_stdin` ends it with exit 0; a second `close_stdin` is `Ok`; a write after close is `session.input-closed` | portable | `crates/substrate-host/tests/ess_confinement_local.rs` |
| T3 | `local`: a write past `limit_bytes` is `session.input-limit`, a 64 KiB + 1 write `session.frame-limit`, a write on a `Null` run `session.not-pipe` | portable | the same |
| T4 | `local`, `Live`: a program writing to both streams yields contiguous offsets per stream, each chunk equal to the record at its offset, the record equal to the delivered bytes, then `None` | portable | the same |
| T5 | `local`, `Live`, `queued_chunks: 1`, a child writing without end and never read: the run ends no sooner than 1 s and within 3 s of its start (the 1 s deadline, the 1 s group settle, 1 s slack); the queued chunk is readable, then `Stalled`, then `None`; `output_stalled` is true; the record begins with the delivered chunk | portable | the same |
| T6 | `local`, `Live`, past the bound: `Bounded { Stdout }` after the last chunk, then `None`; `stdout_truncated`; the record equals the delivered bytes, with no marker | portable | the same |
| T7 | `run_with_stdin`: `cat` returns the input on stdout; a child that exits without reading returns accepted bytes ≤ input and no error; a `Live` profile is `confinement.live-output-unread` and no start record is written | portable | the same |
| T8 | `local`, cancel: a 64 KiB write to `sleep 0.5; wc -c` is cancelled by a 100 ms timeout; the next write is `session.input-closed`, `close_stdin` is `Ok`, and the child reports at most 65 536 bytes and exits 0, so it read end of file | portable | the same |
| T9 | `local`, close during a pending write: with T8's write pending, `close_stdin` has not returned after 200 ms; once the write is cancelled it returns `Ok` | portable | the same |
| T10 | `local`, child exit during a write: a 64 KiB write to a child that exits without reading is `session.write-failed`; so is one to a child whose `setsid` descendant keeps descriptor 0 open and never reads, returned once the run is reported ended | portable | the same |
| T11 | `local`, `Live`, `queued_chunks: 2`, both streams writing and never read: one `Stalled`, after the last chunk of either stream; each stream's chunks contiguous | portable | the same |
| T12 | `linux`: T2, T4, T5, T6, T8, T10 (the plain exit), T11, and `Piped` + `Recorded` with 64 KiB of unread output not stalling | delegated | `ess_confinement_linux.rs` |
| T13 | `systemd` around `local` and around `linux`: T2 and T4 once each, and T5 around `local` (the stall kills the scope's cgroup) | delegated | `ess_confinement_systemd.rs` |
| T14 | `macos`: T2 and T4 | macos | `ess_confinement_macos.rs` |
| T15 | `oci`: T2 with the bytes `0x10 0x11` among the input on the lane's pinned runtime, and T4 | oci | `ess_confinement_oci.rs` |
| T16 | `oci`: a probe naming a runtime version outside `OCI_STDIN_PINNED` proves no `stdin_pipe`, and `Piped` is `confinement.fact-unserved` naming `stdin_pipe` | portable | `oci.rs` unit test |
| T17 | `pod` refuses `Piped` naming `stdin_pipe` and `Live` naming `output_live`, `confinement.fact-unserved`, through `check_profile` | portable | `pod.rs` unit test |
| T18 | the daemon's `PIPE_MAX_INPUT_BYTES` equals `confinement::MAX_STDIN_BYTES` | portable | `crates/substrate-daemon/src/app.rs` unit test |

The scenarios of § 6 run in the lanes that already exist, so no CI job is added. A stall case
costs one stall deadline (1 s) plus the kill; T5, T11 and their `linux` and `systemd` repeats are
the only ones. The default profile changes no existing case.

`tests/confinement-public-items.txt` gains only these lines: `StdinMode`, `OutputDelivery`,
`OutputEvent`, `PipeStream` re-exported, the two profile fields, the four trait methods,
`run_with_stdin`, the two `Fact` variants, `FACT_ORDER`'s new length, `MAX_STDIN_BYTES`,
`STDIN_CHUNK_BYTES`, `MAX_QUEUED_CHUNKS`, `LIVE_OUTPUT_RETENTION_INVALID` and `LIVE_OUTPUT_UNREAD`.
As documentation, the table in `profile.rs` changes, along with the field table in
`website/docs/guides/confinement-profiles.md`. The fact tables in
`website/docs/concepts/adapters.md` and `website/docs/reference/contracts.md` move with the spec,
as `cargo xtask check-docs` requires.

## 10. Decisions

| id | decision | choice | rejected |
|---|---|---|---|
| D1 | stdin surface | profile `StdinMode::Piped { limit_bytes }`, `write_stdin` and `close_stdin`, and the helper `run_with_stdin` | the whole buffer in `Command` (breaks `Command`, no long-lived child); the raw `ChildStdin` (not every adapter holds it) |
| D2 | stdin bounds and pending writes | total 1 to 16 MiB, write 1 to 64 KiB, no adapter buffer, ended by EOF; one shared `StdinWriter`; a cancelled write poisons and closes stdin; `close_stdin` waits for a pending write; a pending write fails when the run ends | unbounded; a bytes-per-second rate bound (memory does not grow with rate); a write deadline inside the adapter (cancellation already bounds it) |
| D3 | live output surface | `OutputDelivery::Live { queued_chunks }`, `read_output(run, timeout)` returning `Chunk`, `Bounded` or `Stalled`, the loss events synthesized from the drain's flags after the queue; `output_stalled(run) -> bool` | polling `output()`; an unbounded channel; a lossy live view; loss events in the queue (no slot when it is full) |
| D4 | backpressure | terminal after the 1 s stall deadline, ADRs 0016 and 0031 unchanged, extended from daemon sessions to in-process embedders | blocking the child; dropping chunks; a configurable deadline |
| D5 | live output and the record | the record begins with the delivered head and equals it unless the run stalled; no marker under `Live`; `Live` with a non-`Head` retention refused (422) | a record with its own retention beside the live stream |
| D6 | ordering and loss | per stream in order, contiguous offsets, no silent loss; no order between streams; the contract counts chunks, not bytes | a total order across stdout and stderr (not observable from two pipes); coalescing to 64 KiB chunks (holds back written lines) |
| D7 | capability statement | two facts, `stdin_pipe` and `output_live`, appended to `Fact` | two new families (changes atlas ADR 0109 decision 8); host-local refusals with no fact (no selection by facts) |
| D8 | refusal names | session codes of `substrate_wire` for per-call refusals; two new codes, `confinement.live-output-retention-invalid` and `confinement.live-output-unread`, on the existing `Driver` variant | a parallel `confinement.*` vocabulary for the same refusals; a new `ConfinementError` variant (a further break) |
| D9 | delivery | one spec story first, then the stdin and live-output stories on one integration branch, merged into `main` only with every lane green; `local`, `linux`, `systemd`, `macos`, `oci` | a spec story per wave; adapter stories that edit the suite (ADR 0044 decision 6) |
| D10 | `pod` | neither capability: stdin needs new Role authority, and live output waits for a merged-stream statement | `pods/attach` now; live output labelled `Stdout` |
| D11 | wire | no change, no bundle | a successor bundle for adapter stdin (the session family already serves it on the wire) |
| D12 | `run_with_stdin` under `Live` | refused `confinement.live-output-unread` before start | draining and discarding the events inside the helper (hides the stream the profile asked for) |
| D13 | `oci` `stdin_pipe` | declared only for runtime and version pairs the oci lane pinned; refused by fact name otherwise | a static declaration for every runtime (a detach sequence could eat bytes on an unlisted one); a probe at open (a later option) |
| D14 | public API | three stated breaks (`Fact`, `FACT_ORDER`, `ConfinementProfile`), new enums `#[non_exhaustive]` | marking `ConfinementProfile` `#[non_exhaustive]` now (breaks every outside literal) |

Open questions for the operator, each with a recommended default:

| id | question | recommended default |
|---|---|---|
| Q1 | Fact names | `stdin_pipe` and `output_live` |
| Q2 | Allow `Live` with `Tail` or `HeadAndTail` later? | refuse now. Lifting it needs a record kept apart from the delivered head, and a consumer that asks |
| Q3 | `pod` stdin through `pods/attach` | defer to its own design, written when a consumer needs it, with the Role change and the WebSocket attach client as its subject |
| Q4 | `oci` stdin on a runtime outside the pinned list | decided by D13: refused by fact name. A probe at open can replace the list later |
| Q5 | Does acceptance need an ADR? | no. ADR 0044 decision 1 has adapters declare facts, ADR 0007 fixes the stdin and close semantics, and ADRs 0016 and 0031 fix the backpressure this design reuses. ADR 0031 was decided for daemon sessions; applying its unchanged deadline and kill to an in-process embedder is recorded here (D4), not in a successor ADR, because neither the rule nor a bound changes. Atlas learns of the two facts from the CHANGELOG, and no atlas file changes |
| Q6 | Should design 30 say it does not publish the two facts? | yes, one sentence in design 30 § 4.3 when it is accepted; not edited here |
| Q7 | A stall visible on `RunObservation` as well as the `Stalled` event | decided by D3: `output_stalled(run)` on the adapter, a non-breaking trait method. A `RunObservation` member stays out: a further public-struct break |
| Q8 | `pod` live output | defer until a design states merged-stream delivery; written when a consumer needs it |

## 11. Stories

1. `story:adapter-io-spec`: every spec change of § 6. No adapter code.
2. `story:adapter-stdin-pipe`, `depends_on` story 1: `StdinMode`, the `StdinWriter`,
   `write_stdin`, `close_stdin`, `run_with_stdin`, `stdin_pipe` on every adapter of § 5 that proves
   it, and the `linux` channel without a live sender (T1 to T3, T7 to T10, and their rows in T12
   to T18).
3. `story:adapter-live-output`, `depends_on` story 1: `OutputDelivery`, the live queue in the
   shared drain and in `drain_capped`'s adapter path, `read_output`, `output_stalled` and
   `output_live` (T1, T4 to T6, T11, and their rows in T12 to T17).

Stories 2 and 3 edit the same adapter files (`confinement/mod.rs`, `local.rs`, `systemd.rs`,
`linux.rs`, `macos.rs`, `oci.rs`, `process.rs`), so they land on the integration branch one after
the other, not in parallel. The consumer's move from `HostDriver` sessions to the adapter is its
own change after its pin moves, and none of it is done here.

## 12. Out of scope

- A pty on any adapter, and unrecorded capture ([design 23](23-terminal-capture-selection.md)) on
  any adapter.
- `pod` standard input and `pod` live output (D10, Q3, Q8).
- A probe at open for `oci` standard input (D13).
- Any daemon route, wire type or bundle.
- A `b10x-substrate confine run` flag that forwards its own stdin. That can follow as its own
  change.
- Reconnecting to a run after the embedding process died. Runs remain in-process, as today.

## 13. Review pass 1

An independent review of the first draft returned needs-revision with eight blocking findings and
five notes. Where each is answered:

| finding | answer |
|---|---|
| F1 the record is not what was delivered (marker; filling after a stall) | § 1 rows on `drain_capped`; § 3 *The record holds the delivered head*; § 5 `linux` row (`mark_truncation = false`); D5; T4, T6 |
| F2 no mechanism for loss events | § 3 *Loss events are synthesized after the queue*; § 5 `local` and `linux` rows (flags, accessor); D3; `Interrupted` removed with `pod` live output |
| F3 cancellation has no mechanism | § 2 *One writer per run* and *Cancellation closes*; D2; T8 |
| F4 pending writes | § 2 *Pending writes*; D2; T9, T10 |
| F5 `run_with_stdin` under `Live` stalls | § 2 `run_with_stdin` (refused `confinement.live-output-unread`; full duplex needs a concurrent reader); D12; T7 |
| F6 static `oci` fact for a lane result | § 5 `oci` row (`OCI_STDIN_PINNED`); D13; Q4; T15, T16 |
| F7 public API breaks understated | § 1 last row; § 4 *Public API compatibility*; D14; § 9 public items |
| F8 stories cannot land independently | § 6 *One spec story, first*; D9; § 11 (`depends_on` edges) |
| F9 stall threshold and memory | § 3 *Chunk sizes in this tree* and *Unread output*; D6 |
| F10 Q7 fails in the stall case | `output_stalled` in § 3; D3; Q7; T5 |
| F11 `pod` merged stream labelled `Stdout` | § 5 `pod` row: `pod` live output deferred; D10; Q8; T17 |
| F12 missing cases; T5's time bound | T5 (within 3 s), T8 cancel, T9 close during a pending write, T10 child exit during a write, T11 stall with both streams, T13 stall on `systemd` |
| F13 ADR 0031 extended to non-session embedders | § 3 *Backpressure is terminal*; D4; Q5 |
