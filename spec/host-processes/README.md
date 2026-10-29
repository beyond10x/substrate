# Host processes

The ESS domain `substrate.host_processes` specifies the resource family of
[ADR 0036](../../adr/0036-host-processes-are-a-supervised-resource-family.md): the *host process*
(supervised process), a long-lived process tree that a session starts through the daemon, with an
owner, a reason, a lease or a reasoned keep, an optional hard deadline, one cgroup, observations, a
bounded log and an event stream. Under the `host` profile the process runs unconfined, and the
record says so (`confinement: none`).

The wire types of S1-S7, S11 and S12 are declared in `wire.yaml` and generated from it into
`crates/substrate-wire/src/generated/host_process.rs`, which `substrate-wire` re-exports (as
`ProcessStartInput`, `ProcessRecord`, `ProcessState`, `ProcessEndedBy` and the rest). No route or wire
bundle is derived from this domain; the bundles that carry the family (`0.20.0` for S1-S7 and S11,
`0.24.0` for S12) are cut by hand from the same shapes.
`cargo xtask check-ess` validates it, compares the committed synthesized suite
[`../host-processes-suite.json`](../host-processes-suite.json) byte for byte, runs the synthesized
and authored scenarios against the ESS interpreted reference, and regenerates the wire types and
compares them byte for byte.

```console
ess specify validate --path spec/host-processes --strict-requires
ess verify conform synthesize --path spec/host-processes --strict-requires \
  --out spec/host-processes-suite.json
ess verify conform run --path spec/host-processes --scenarios spec/host-processes \
  --target interpreted --report-format 2 --strict-requires
```

The pin is ESS 0.76.0 (`ess-inputs.yaml`). The authored scenarios under `scenarios/` are the
acceptance set, one or more per command. A run against the interpreted reference checks the model
only. It is not evidence that any implementation conforms.

## How the open points were settled

ADR 0036 left sixteen points for the specification. The wave decision of 2026-10-17 fixes one
option for each, the one that can be undone, and each is a `DECIDED (point n)` comment beside the
place it governs:

- **Modelled:** stream delivery at least once with a per-process cursor (1), refused appends
  dropped (2), the subject as the only authority, with the daemon's supervisor as its own actor (6),
  owner-name uniqueness among non-terminal records of one owner session (7), cumulative CPU at
  observation time (13), the byte-bounded log that discards its oldest bytes and records how many
  (14), and an append for an unknown process refused inside the daemon (15).
- **Implementation-side, because ESS 0.60.0 cannot state it:** the lease expiry instant (4), the
  retention sweep that removes ended records (5), the supervisor clock that fires lease expiry and
  deadlines (8, 9), and the cursor's monotonicity (16), which the ordering scenarios check. The
  same holds for three values of ADR 0036 itself: the effective deadline `deadline_at`, the earlier
  of `timeout_ms` and `expires_at` (S11), which the supervisor names when it enforces it; the
  environment digest `env_digest` (S1); and the wire bounds `MIN_LEASE_TTL_MS`,
  `MAX_LEASE_TTL_MS` and `MAX_PROCESS_TIMEOUT_MS`, which ESS 0.60.0 cannot declare as constants.
- **Deferred to the S8/S9 wave:** the profile of container and adopted records (3), adopted
  supervision (10), the container label keys (11) and a container timeout (12). The `StartContainer`
  and `AdoptProcess` declarations stay drafted and are marked deferred; nothing in this wave builds
  them.

## What this model does not cover

The kernel half of these claims belongs to a native suite. That covers killing the whole cgroup on
every stop, the process tree being gone before a record ends, the refusal of the `host` profile
under a hosted or static-bearer TCP posture, the per-subject partition of records, and the byte
bound on the log file. Here a record reaches `Ended` only through the supervisor's empty-cgroup
report (`RecordExit`) or its reconciliation finding the cgroup gone (`RecordVanished`, which ends
the record `vanished`). Lease expiry and the hard deadline are supervisor commands, because ESS has
no clock-driven transition.

## Wire types (`wire.yaml`)

`wire.yaml` declares what the routes of S1-S7 and S11 carry inside the existing mutation and success
envelopes: the start input, the record and its lease, deadline, exit and observations, the listing
page, the stop-by-owner input and result, and the log tail. `cargo xtask check-ess` generates them
with ESS's Rust target (`--names short`) and compares the file byte for byte. Three choices follow
from what ESS 0.60.0 can state:

- **State spelling.** ESS requires a lifecycle state name to start with an upper-case letter, and the
  wire spells every resource state in lower case, so `ProcessState` (`starting`, `running`,
  `stopping`, `ended`) is its own enum beside the `HostProcess` lifecycle.
- **Lease request.** ESS realizes a union with an explicit tag member, which ADR 0036's
  `{ "ttl_ms": n }` or `{ "keep": true }` does not have, so `ProcessLeaseRequest` is one struct with
  optional members, and the daemon admits exactly one of a TTL and a keep.
- **Reachable refusals.** Every start member with a named refusal is optional, and `profile` and
  `expires_at` are text, so an omitted or malformed member reaches its named refusal
  (`process.profile-required`, `process.profile-unknown`, `process.expires-at-invalid`, …)
  instead of the generic schema refusal.

Hand-written beside them in `crates/substrate-wire/src/lib.rs`, because ESS 0.60.0 cannot express
them: the bounds (`MAX_PROCESS_*` and the retention and grace constants; ESS declares no constants),
the refusal codes with their statuses (`PROCESS_REFUSALS`; an ESS error carries no wire code or
status), the `process.host` capability fact (a member of the existing hand-written capability
facts), and the query-string types `ProcessListQuery`, `ProcessLogsQuery` and `ProcessEventsQuery`
(ESS realizes an `Integer` as a JSON number, which a query-string decoder cannot read).

## The per-process event stream (S12)

`wire.yaml` adds `ProcessTransition`, the observation of the seven `process.*` variants of the event
union, and `ProcessStreamItem`, one frame of `GET /v1/processes/{process_id}/events`. ADR 0036 left
these points open; each takes the option that keeps the stream honest, refusing or reporting a gap
and never skipping silently:

- **Order.** Each `process.*` event is placed in the stream at the log byte offset the S7 log had
  reached when the event was recorded, so a line written before a transition precedes it.
  `process.started` precedes every line when the event log still retains it: while the record is
  `starting` the stream holds the lines the program already wrote, and sends none before the
  record is running or has ended. The item carries no per-process `seq` of its own: the opaque
  cursor is the order, and a lifecycle item carries its event's `seq`.
- **Cursor.** The cursor encodes the record it belongs to, the event-log source, generation and
  last delivered `seq`, and the log byte offset. A cursor the daemon cannot honour is refused
  before the upgrade with the event log's own codes (409): `event.source-mismatch` for another
  source or generation, `event.retention-gap` when event history it needs is no longer retained,
  `event.cursor-invalid` when it does not match the published `process-event-cursor` pattern,
  names another record, is beyond the event barrier, or beyond the end of the log.
- **Gap.** A cursor, or the start of a stream without one, before the oldest retained log byte
  yields one `gap` item stating how many bytes the bound discarded, then the oldest retained line.
- **Terminal item.** `ended` follows the record's end, every retained event and the whole retained
  log, then the daemon closes the socket. A stream it can no longer serve (the one-hour lifetime
  the event stream also has, or a failed read) closes with WebSocket status 1011 and no `ended`
  item; the reader reconnects with its last cursor.
- **Readers.** `MAX_PROCESS_EVENT_READERS` (8) concurrent streams of one process, and, over all of
  a subject's records, live or ended, `MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT` (512, the readers
  bound of the `MAX_PROCESSES_PER_SUBJECT` live records) per subject and
  `MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT` (512) per deployment; past any of them the next is
  `process.events-readers-exceeded` (429, retriable) before the upgrade. From the upgrade on, the
  socket is counted by those bounds and its transport slot returns, as every upgraded stream's
  does (ADR 0042). An idle stream looks at its log's end every 100 ms and reads the store when
  the log grew, an event was committed in its subject, or a second has passed: at most one store
  read per second.
- **Slow reader.** The stream reads the log file, never the program's pipes, so a reader that does
  not drain its socket stalls only its own stream. A send that does not complete within the send
  bound the event stream uses (5 s) drops the connection without an `ended` item; the reader
  resumes from its last cursor.
- **Placement.** The store records beside each `process.*` event the log offset the record's log
  had reached when the event was decided: `process.started` at 0, a renewal, a lease expiry or a
  deadline at the log's end at that moment, an end event none (it follows the whole log). The
  stream sends an event once it has delivered the log up to that offset.
- **Cursor form.** `pe1.<process>.<source>.<generation>.<seq>.<offset>`: `process` is the record
  id (its alphabet has no `.`), `seq` the event-log seq through which every event of the record
  has been delivered, `offset` the log offset after the last delivered line. The daemon reads
  exactly the published pattern (`substrate_wire::ProcessEventCursor`); anything else is
  `event.cursor-invalid`.
- **Without a cursor** (ADR 0036 S12). A stream opened without a cursor is never refused for
  retention: it delivers what the event log and the log still hold, then, once the record has
  ended, the terminal item, and closes; on a running record it starts from what is held and
  continues live. `event.retention-gap` is only for a cursor whose seq retention has passed.
- **Capability.** No new fact. Like `process.logs.get` the route carries no capability predicate: a
  daemon that supervises nothing still serves the stream of a record an earlier run left, from its
  stored log.
- **Lines.** `text` is at most `MAX_PROCESS_EVENT_LINE_BYTES` (16 KiB), cut at a character
  boundary; `truncated: true` says the delivered text is shorter than the line, whether the line
  was longer than the bound or its replaced text outgrew it, and `full_bytes` is the length the
  program wrote.

The model states the stream's order and its single terminal item. Four authored scenarios carry the
story's names: `host-process-stream-interleaves-events-and-lines-in-order`,
`host-process-stream-ends-with-exactly-one-terminal-item`,
`host-process-stream-terminal-item-names-ended-by` and
`host-process-stream-on-an-ended-record-replays-and-closes`. The rest of S12 is not a model fact
and runs against the daemon in `crates/substrate-daemon/tests/processes.rs` under the same names:
resuming from a cursor (a generated `seq` cannot be captured into a later step), the global event
log's variants, output lines never entering it, the gap, truncation, lossy text, a slow reader and
the readers bound.
