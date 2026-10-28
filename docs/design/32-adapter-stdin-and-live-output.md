# Design 32: standard input and live output on the confinement adapters

Status: proposed, 2026-10-28. No story owns it yet; § 11 proposes the stories. It changes no wire
byte and no contract bundle: the adapters are substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). No code before acceptance (invariant 8).

Two consumer asks, both from the Loom repository's child launcher
(`crates/loom-intake-slice/src/confinement.rs` in the loom repository, `run_child` with standard
input and `spawn_child`):

1. A confinement adapter run takes standard input. Today every adapter gives the run `/dev/null`.
2. A run's output reaches the embedder while the run runs. Today it is read after the run ends.

Loom gets both today only from a raw-pipe session on `HostDriver` (`start_pipe_session`,
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
| `pod` renders `"stdin": false, "tty": false` and reads the log once the container terminated, `limitBytes` = bound + 1 | `pod.rs` (the container spec, `read_log`) |
| `local`, `macos`, `systemd` around `local` and `oci` drain both pipes into one shared `Captured` through `output_retention::drain`; `output()` while running returns a snapshot. `linux` reports output only once the run ended | `crates/substrate-host/src/output_retention.rs`; design 29 § 6 |
| the host already serves live stdin and output for sessions: `ChildChannel::Pipes`, `write_pipe` (frame ≤ `frame_limit`, total ≤ `input_limit`, refusals `session.frame-limit`, `session.input-limit`, `session.input-closed`, `session.write-failed`), `close_pipe_input` (half-close), `read_pipe` (`session.read-timeout`, `session.timeout-invalid`) | `process.rs` |
| live output is a bounded queue: frames ≤ `PIPE_FRAME_BYTES` (64 KiB), at most `PIPE_QUEUED_FRAMES` (16); a frame not reserved within `PIPE_OUTPUT_STALL_TIMEOUT` (1 s) records `session.output-backpressure` and ends the tree; the drain continues so the child never blocks; forwarding stops at the stream's output bound | `process.rs` `drain_capped`, `forward_frame`; [ADR 0016](../../adr/0016-pipe-output-backpressure-is-terminal.md), [ADR 0031](../../adr/0031-live-output-backpressure-has-a-stall-deadline.md) |
| with a live sender, the recorded output is the head of what was forwarded, whatever retention was asked | `drain_capped`; design 29 § 6 |
| the daemon caps a session's total input at `PIPE_MAX_INPUT_BYTES` (16 MiB) | `crates/substrate-daemon/src/app.rs` |
| `ConfinementAdapter`, `ConfinementProfile`, `RunOutput`, `Fact` and `ContractFamily` are public; `Fact` is generated from the ESS domain and not `#[non_exhaustive]` | `confinement/mod.rs`, `confinement/profile.rs`, `crates/substrate-host/src/generated/confinement.rs`, `crates/substrate-host/tests/confinement-public-items.txt` |

The wire already serves both capabilities for the host driver: the session family
(`/v1/sessions`, [ADR 0007](../../adr/0007-protocol-processes-use-raw-pipe-sessions.md), ADR 0028)
in `crates/substrate-daemon/src/app/sessions.rs`.

## 2. Standard input: the adapter surface

### Options

| | option | cost |
|---|---|---|
| A | the whole input in `Command` at start (`stdin: Option<Vec<u8>>`), written and closed by the adapter | serves `run_child` only, not a long-lived child (`spawn_child`, a `cat-file --batch` reader). `Command` is a public struct with no `Default`, so every literal breaks |
| **B** | **a profile mode plus three trait methods**: the profile asks for a piped stdin and its total bound; `write_stdin`, `close_stdin` on the run; a free helper writes a whole buffer and closes | serves both asks; admission refuses by fact name before any start; `Command` is unchanged |
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
  are unaffected. A write after it is `session.input-closed` (422). A run that ends closes it.
- **No buffering.** A write goes straight to the pipe, at most one chunk in flight per run, and
  returns once the pipe took every byte. A child that does not read blocks the writer, not the
  host. A failed write (the child closed its stdin or ended) is `session.write-failed` (500) and
  never reported as delivered.
- **Cancellation closes.** A `write_stdin` future dropped before it returns may have written part
  of its chunk, so the adapter closes stdin. The next write is `session.input-closed`, never a
  silent splice.
- **No rate bound beyond the pipe.** The writer is the embedder, which already chooses the argv;
  host memory is bounded by the one chunk in flight, and the child's read rate paces the writer.
  The reserved `rate_bounds` field (ADR 0044 decision 5) stays reserved.
- **Wrong run.** `write_stdin` or `close_stdin` on a run admitted with `Null` is `session.not-pipe`
  (422). An unknown run is the adapters' existing not-found.
- **`run_with_stdin`** writes in 64 KiB chunks while it waits, and stops writing without failing
  when the child closed its stdin or ended first (`session.write-failed`): the child read what it
  read. It returns the number of bytes the pipe accepted. Under `OutputDelivery::Recorded` the
  adapter drains output on its own, so writing all input before reading output cannot deadlock.
- **No terminal.** Stdin is a pipe, never a pty. The pty mode ([ADR 0019](../../adr/0019-pty-is-a-second-session-mode.md),
  [design 13](13-pty-sessions.md)) stays a daemon session mode.

The per-call refusals reuse the session codes of `substrate_wire` because `linux` passes the host
driver's answers through unchanged; a second spelling for the same refusal would let a client branch
on the adapter (invariant 4). They are answers to a call, not admissions or starts, so they write no
audit record, as `signal` errors write none.

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
    /// The stream reached its output bound; later bytes are drained and not delivered.
    Bounded { stream: PipeStream },
    /// No chunk was taken within the stall deadline: delivery stopped and the run is ended
    /// (`session.output-backpressure`).
    Stalled,
    /// Delivery of the stream stopped before its end for another named reason (§ 5, `pod`).
    Interrupted { stream: PipeStream, code: &'static str },
}

// On `ConfinementAdapter`, default body `session.not-pipe`:
/// The next event, waiting at most `timeout`; `None` once both streams ended and every event
/// was read.
async fn read_output(&self, run: &RunId, timeout: Duration)
    -> Result<Option<OutputEvent>, ConfinementError>;
```

`PipeStream` (`Stdout`, `Stderr`) is the host's existing type, re-exported from `confinement`. The
wire's `OutputStream` is a different type and is not reused.

### Rules

- **Bounded.** `queued_chunks` is 1 to `MAX_QUEUED_CHUNKS` (16, the host's `PIPE_QUEUED_FRAMES`);
  outside it, `session.limit-unserved` (429) at admission. A chunk holds 1 to 64 KiB, and its
  boundaries carry no meaning. Each stream delivers at most its existing output bound (1 MiB, or
  `io_bounds.output_bytes` where `limits` is served). No bound changes.
- **Backpressure is terminal after the stall deadline**, as ADR 0031 decides for sessions: the
  adapter waits at most `PIPE_OUTPUT_STALL_TIMEOUT` (1 s) for a queue slot, then stops delivery and
  ends the run as its `signal(Kill)` does (whole tree where the adapter proves `whole_tree_kill`,
  the process group or container otherwise). The drain continues, so the child never blocks on its
  pipes. Already queued chunks stay readable, followed by `Stalled`.
- **Ordering and loss.** Within one stream: in order, contiguous (`offset` + `bytes.len()` is the
  next chunk's `offset`), no duplicate, no gap. Between stdout and stderr: no order is promised.
  A pipe pair has none to observe. A loss is always a named event (`Bounded`, `Stalled`,
  `Interrupted`), after the stream's last chunk and before `None`, never in place of a chunk.
- **The record is what was delivered.** With `Live`, `output()` after the run holds each stream's
  head within the bound, and for every chunk `stdout[offset..offset + len] == bytes`. That is
  `drain_capped`'s existing rule ("a session's stored bytes are what it forwarded"), extended to the
  shared `drain`. `KeptOutput` reports `Head`, and `*_truncated` keeps its meaning. A profile asking
  `Live` together with `Tail` or `HeadAndTail` is refused `confinement.live-output-retention-invalid`
  (422), because the record could not then equal the delivered bytes (§ 10, Q2).
- **Unread output.** A `Live` run whose embedder does not read stalls once more than
  `queued_chunks` × 64 KiB wait, and ends `Stalled`. An embedder that only wants the record asks
  for `Recorded`, which never stalls. Events still queued when the run ends stay readable until
  the adapter forgets the run (its existing 128-run tracking bound). Per run, that adds at most
  `queued_chunks` × 64 KiB of memory to the record's bound.
- **Timeouts.** A zero timeout is `session.timeout-invalid` (422), and an elapsed one
  `session.read-timeout` (500), as `read_pipe`. `read_output` on a `Recorded` run is
  `session.not-pipe` (422).
- **`wait` and `output` are unchanged.** `wait` returns when the run ended, whether or not its
  events were read.

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

Adding two variants to the exhaustive `Fact` enum breaks a consumer that matches it exhaustively.
All consumers pin an exact revision, and the crate is 0.x, so the break ships in the next minor with
a CHANGELOG line. The two new profile fields are additive for `..ConfinementProfile::default()`.

## 5. Per-adapter feasibility and order

| adapter | `stdin_pipe` | `output_live` | mechanism | wave |
|---|---|---|---|---|
| `local` | proved | proved | `Stdio::piped()` for stdin; `output_retention::drain` gains an optional live queue that it fills before the record; the stall kills the process group | 1 |
| `systemd` around `local` | proved | proved | its own spawn, the same code as `local`; the stall kills the run's cgroup | 1 |
| `linux` | proved with the floor | proved with the floor | the host driver's session path. `AdapterStart` gains `stdin: bool` and `live: Option<queue>` (`pub(crate)`). `ChildChannel::Pipes` without a live sender is new: stdin piped, output recorded through `drain_capped` with no sender, so `Piped` + `Recorded` never stalls. `write_pipe`, `close_pipe_input` and `read_pipe` serve the adapter by exec id. Sessions keep their path unchanged | 1 |
| `systemd` around `linux` | as `linux` | as `linux` | delegates to `linux` | 1 |
| `macos` | proved once the macos lane case passes | the same | the same `Captured` path as `local`. Seatbelt rules govern opens by path, and the run inherits descriptors 0 to 2. The macos lane proves that a confined `cat` reads and writes them before the fact is declared (ADR 0023's rule: nothing published unproved) | 2 |
| `oci` | proved once the oci lane case passes on the runtime | proved | `create --interactive` and `start --attach --interactive` with the runtime client's stdin piped; output is already the attached client's two pipes. The runtime's detach sequence (`ctrl-p ctrl-q`) must not eat input bytes: the adapter disables it where the runtime has an option for it, and the lane case writes the bytes `0x10 0x11` through `cat` and reads them back. On a runtime where that fails, `stdin_pipe` is not declared | 2 |
| `pod` | **not served** | proved | stdin needs `stdin: true` and `stdinOnce: true` in the Pod and the `pods/attach` subresource on the Role: new authority (attach to any Pod in the namespace) and a new streaming client. Refused `confinement.fact-unserved` naming `stdin_pipe`. Live output uses `pods/log?follow=true&limitBytes=<bound + 1>`, the same `get` on `pods/log` the Role already has, read as chunks on `Stdout` (the kubelet merges the streams, design 28). A follow connection that drops before the container terminated is re-read from the start with the same `limitBytes`, skipping `offset` bytes; a re-read shorter than `offset` ends delivery with `Interrupted` naming `confinement.pod-api-failed`. The post-run log read stays the record | 3 |

Wave 1 is what the consumer runs on: Linux, with `linux` or `systemd`, and `local` for
development. Waves 2 and 3 are independent of each other.

## 6. The ESS confinement domain and suite

Changes in `spec/confinement/confinement.yaml`:

- `Fact` gains `stdin_pipe` and `output_live`, with a comment line each, after `image_digest_pinned`.
- `AdmitProfile` gains the outcomes `fact-unserved-stdin-pipe` and `fact-unserved-output-live`,
  after `fact-unserved-image-digest-pinned`.
- The declared sets above `OpenAdapter` gain the facts as § 5 states, each with its source.

The model does not see `limit_bytes`, `queued_chunks` or the retention conflict: like design 29's
refusals, they are raised by an adapter's `check`, which `AdmitProfile` does not model, and are held
by the Rust cases of § 9. Design 29 § 10 records that gap, and this design does not close it.

New authored scenarios, named by the lane rule of `spec/confinement/README.md`:

| scenario | lane | wave |
|---|---|---|
| `local-admits-stdin-pipe-and-output-live-on-portable` | portable | 1 |
| `linux-admits-stdin-pipe-and-output-live-on-delegated` | delegated | 1 |
| `systemd-local-admits-stdin-pipe-and-output-live-on-delegated` | delegated | 1 |
| `systemd-linux-admits-stdin-pipe-and-output-live-on-delegated` | delegated | 1 |
| `model-stdin-pipe-and-output-live-unserved-on-interpreted` (an adapter set without either fact, refused by each name) | interpreted | 1 |
| `macos-admits-stdin-pipe-and-output-live-on-macos` | macos | 2 |
| `oci-admits-stdin-pipe-and-output-live-on-oci` | oci | 2 |
| `pod-stdin-pipe-unserved-on-pod` | pod | 3 |
| `pod-admits-output-live-on-pod` | pod | 3 |

Every lane runs every authored scenario of its adapter and compares the adapter's real facts with
the scenario's open step. Adding a fact to an adapter therefore also edits the open step of every
authored scenario that opens that adapter (at the time of writing, 11 files name `local`, 11 `linux`
and 7 `systemd`). So the spec change of a wave cannot land ahead of that wave's adapter code
without turning its lane red. ADR 0044 decision 6 keeps adapter stories out of the suite, so
each wave has its own spec story that lands in the same wave as its adapter stories, spec commit
first. `cargo xtask check-ess` regenerates `spec/confinement-suite.json` and `Fact`. The README's
counts (lanes, synthesized and refused obligations) move with it.

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
  runtime keeps no copy. `pod` gains no Role verb: stdin is refused, and live output uses the
  `pods/log` access the adapter already proves at open.
- **Stdin bounds.** At most 16 MiB in total per run, 64 KiB per write, one write in flight, nothing
  buffered by the adapter. No rate bound: memory does not grow with rate (§ 2).
- **Output bounds unchanged.** The same per-stream byte bound, the same 1 s stall deadline, a queue
  of at most 16 × 64 KiB per run, and terminal backpressure as ADRs 0016 and 0031 state. Under
  `Live` the record stays inside the same bound, because it is the delivered head.
- **Declared insecure stays declared.** `local` gains a pipe, not confinement. Its admission still
  needs a reason, and every observation still carries it.
- **No silent degradation.** An adapter that cannot serve either capability refuses it by fact
  name at admission. A lost byte is a named event.

## 9. Tests and CI cost

Each case runs its scenario once and finishes in seconds. Linux cases lower
`io_bounds.output_bytes` to 4096. Fixed-bound adapters write just past 1 MiB, as design 29's
retention cases already do. Each case goes into its adapter's existing lane test file, and no test
binary is added. Each delegated case prints `<name>: executed` for `require_executed` in
`scripts/delegated-lane.sh`.

| # | case | lane | file |
|---|---|---|---|
| T1 | `check_streams`: `limit_bytes` 0 and above 16 MiB, `queued_chunks` 0 and 17 are `session.limit-unserved`; `Live` with `Tail` is `confinement.live-output-retention-invalid`; each writes one `profile_refused` record | portable | `confinement/mod.rs` unit tests |
| T2 | `local`: `cat` with `Piped` echoes three writes on `Stdout`; `close_stdin` ends it with exit 0; a write after close is `session.input-closed` | portable | `crates/substrate-host/tests/ess_confinement_local.rs` |
| T3 | `local`: a write past `limit_bytes` is `session.input-limit`, a 64 KiB + 1 write `session.frame-limit`, a write on a `Null` run `session.not-pipe` | portable | the same |
| T4 | `local`, `Live`: a program writing to both streams yields contiguous offsets per stream, each chunk equal to the record at its offset, then `None` | portable | the same |
| T5 | `local`, `Live`, `queued_chunks: 1`, never read: the run ends by kill within seconds; the queued chunk is readable, then `Stalled`, then `None` | portable | the same |
| T6 | `local`, `Live`, past the bound: `Bounded { Stdout }` after the last chunk; `stdout_truncated` | portable | the same |
| T7 | `run_with_stdin`: `cat` returns the input on stdout; a child that exits without reading returns accepted bytes ≤ input and no error | portable | the same |
| T8 to T12 | `linux`: T2, T4, T5 and T6, and `Piped` + `Recorded` with 64 KiB of unread output does not stall | delegated | `ess_confinement_linux.rs` |
| T13 | `systemd` around `local` and around `linux`: T2 and T4 once each | delegated | `ess_confinement_systemd.rs` |
| T14 | `macos`: T2 and T4 | macos | `ess_confinement_macos.rs` |
| T15 | `oci`: T2 with the bytes `0x10 0x11` among the input, and T4 | oci | `ess_confinement_oci.rs` |
| T16 | `pod` refuses `Piped` `confinement.fact-unserved` naming `stdin_pipe` through `check_profile` | portable | `pod.rs` unit test |
| T17 | `pod`, `Live`: a container printing two lines with a pause between them delivers the first before the container ends | pod | `ess_confinement_pod.rs` |
| T18 | the daemon's `PIPE_MAX_INPUT_BYTES` equals `confinement::MAX_STDIN_BYTES` | portable | `crates/substrate-daemon/src/app.rs` unit test |

The scenarios of § 6 run in the lanes that already exist, so no CI job is added. A stalled case
costs one stall deadline (1 s). The default profile changes no existing case.

`tests/confinement-public-items.txt` gains only these lines: `StdinMode`, `OutputDelivery`,
`OutputEvent`, `PipeStream` re-exported, the two profile fields, the three trait methods, `run_with_stdin`,
the two `Fact` variants, `MAX_STDIN_BYTES`, `STDIN_CHUNK_BYTES`, `MAX_QUEUED_CHUNKS` and
`LIVE_OUTPUT_RETENTION_INVALID`. As documentation, the table in `profile.rs` changes, along with
the field table in `website/docs/guides/confinement-profiles.md`. The fact tables in
`website/docs/concepts/adapters.md` and `website/docs/reference/contracts.md` move with the spec,
as `cargo xtask check-docs` requires.

## 10. Decisions

| id | decision | choice | rejected |
|---|---|---|---|
| D1 | stdin surface | profile `StdinMode::Piped { limit_bytes }`, `write_stdin` and `close_stdin`, and the helper `run_with_stdin` | the whole buffer in `Command` (breaks `Command`, no long-lived child); the raw `ChildStdin` (not every adapter holds it) |
| D2 | stdin bounds | total 1 to 16 MiB, write 1 to 64 KiB, no adapter buffer, ended by EOF, a cancelled write closes stdin | unbounded; a bytes-per-second rate bound (memory does not grow with rate) |
| D3 | live output surface | `OutputDelivery::Live { queued_chunks }` and `read_output(run, timeout)` returning `OutputEvent` | polling `output()`; an unbounded channel; a lossy live view |
| D4 | backpressure | terminal after the 1 s stall deadline, ADRs 0016 and 0031 unchanged | blocking the child; dropping chunks; a configurable deadline |
| D5 | live output and the record | the record is the delivered head; `Live` with a non-`Head` retention refused (422) | a record with its own retention beside the live stream |
| D6 | ordering and loss | per stream in order, contiguous offsets, no silent loss; no order between streams | a total order across stdout and stderr (not observable from two pipes) |
| D7 | capability statement | two facts, `stdin_pipe` and `output_live`, appended to `Fact` | two new families (changes atlas ADR 0109 decision 8); host-local refusals with no fact (no selection by facts) |
| D8 | refusal names | session codes of `substrate_wire` for per-call refusals; one new `confinement.live-output-retention-invalid` | a parallel `confinement.*` vocabulary for the same refusals |
| D9 | first adapters | wave 1 `local`, `linux`, `systemd` (both inners); wave 2 `macos`, `oci`; wave 3 `pod` live output | every adapter at once |
| D10 | `pod` stdin | not served, `confinement.fact-unserved` | `pods/attach` now (new Role authority and a new streaming client) |
| D11 | wire | no change, no bundle | a successor bundle for adapter stdin (the session family already serves it on the wire) |

Open questions for the operator, each with a recommended default:

| id | question | recommended default |
|---|---|---|
| Q1 | Fact names | `stdin_pipe` and `output_live` |
| Q2 | Allow `Live` with `Tail` or `HeadAndTail` later? | refuse now. Lifting it needs a record kept apart from the delivered head, and a consumer that asks |
| Q3 | `pod` stdin through `pods/attach` | defer to its own design, written when a consumer needs it, with the Role change and the WebSocket attach client as its subject |
| Q4 | `oci` stdin on a runtime whose detach sequence cannot be disabled | do not declare `stdin_pipe` on that runtime; the lane case decides per runtime |
| Q5 | Does acceptance need an ADR? | no. ADR 0044 decision 1 has adapters declare facts, ADR 0007 fixes the stdin and close semantics, and ADRs 0016 and 0031 fix the backpressure this design reuses. Atlas learns of the two facts from the CHANGELOG, and no atlas file changes |
| Q6 | Should design 30 say it does not publish the two facts? | yes, one sentence in design 30 § 4.3 when it is accepted; not edited here |
| Q7 | A stall visible on `RunObservation` (a `refusal` member) as well as the `Stalled` event | the event only. The embedder that stalled the run is the one reading the stream; a new `RunObservation` member is a further public-struct break |

## 11. Proposed stories

1. `spec`: the two facts, the two outcomes and the wave 1 open sets and scenarios (§ 6).
2. `local` and `systemd` around `local`: the shared `drain` live queue, `check_streams`, the trait
   methods and `run_with_stdin` (T1 to T7, and T13 for `local`).
3. `linux` and `systemd` around `linux`: `AdapterStart` and the stdin-without-live channel in the
   host driver (T8 to T13).
4. Wave 2: `macos` and `oci`, each with its spec story.
5. Wave 3: `pod` live output, with its spec story.

Stories 1 to 3 are one wave. Loom's switch from `HostDriver` sessions to the adapter is Loom's own
change after its pin moves, and none of it is done here.

## 12. Out of scope

- A pty on any adapter, and unrecorded capture ([design 23](23-terminal-capture-selection.md)) on
  any adapter.
- Any daemon route, wire type or bundle.
- A `b10x-substrate confine run` flag that forwards its own stdin. That can follow as its own
  change.
- Reconnecting to a run after the embedding process died. Runs remain in-process, as today.
