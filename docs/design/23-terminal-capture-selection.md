# Design 23: policy-controlled terminal capture

**Status:** proposed implementation design · **Date:** 2026-10-02

## Requirement and boundary

Issue #112 requests live PTY and pipe output without durable terminal payload. This applies to
Substrate-owned capture, SQLite/WAL, operation/event payloads and diagnostic logs. It does not
control files deliberately written by the child, the client, swap, core dumps or host administration.
Confinement, authenticated attachment, resource measurements, leases and whole-tree cancellation
continue to apply. No encryption protocol is introduced.

## Admission and compatibility

A session start gains optional `capture: recorded|unrecorded`, defaulting to recorded. The SDK omits
the default for older daemons. Unrecorded is never retried as recorded. The daemon operator must
explicitly enable `--allow-unrecorded-sessions`; existing execution/subject authorization remains
required. A missing driver fact `sessions.unrecorded` is `session.capture-unserved`, and disabled
operator policy is `session.capture-disallowed`, both before dispatch. The capability document
reports supported capture modes, effective policy and fixed attachment deadlines/capacity.

The initial durable session and exec metadata record the selected mode before dispatch. Observations
carry unrecorded mode and content-free byte counters; legacy recorded observations omit the new
optional fields. Output-query returns `exec.output-unrecorded`, not an empty transcript. Single-use
attachment and kill-on-disconnect remain unchanged; there is no reconnect replay and recovery never
manufactures content. An unsupported older daemon/driver cannot silently downgrade the request.

## Accounting and termination

The existing finite execution `output_bytes` is a per-stream byte ceiling for pipes and a merged
ceiling for PTY. Counters operate on raw child bytes before JSON/base64 framing, independent of
retained bytes. Observed bytes count reads (including bytes discarded while containing a process);
queued bytes count successful host-channel enqueue, not client receipt. Queue high-water is an
observed occupancy, bounded by the admitted frame count. There is no claim of acknowledged client
delivery. Frame size, input byte limit and queue frame count remain independent. Crossing the
output ceiling in unrecorded mode cancels the whole process tree and names the bound. Queue full,
closed reader and transport failure contain the process. The existing 5-second send deadline,
one-hour attachment lifetime, 32 global attachment slots and one-use attachment remain finite.

The host never appends unrecorded bytes to the capture accumulator, including truncation markers.
Only bounded live buffers hold them. The store rejects a non-empty capture for an unrecorded exec
before executing SQL, and rejects a mode change relative to the durable reservation. This applies
through completion, observation, maintenance, cancellation and lease/recovery paths. A persistence
failure retains content-free terminal metadata for retry; it cannot activate recording.

## Contract and evidence

Cut successor development bundle 0.17.0; preserve every byte in 0.1.0 through 0.16.0. Document SDK
selection, named refusals and no-replay behavior. The ESS session projection names the capture
contract; native conformance scenarios exercise production driver/store paths.

Acceptance scenarios: default-recording; policy-refused; driver-unserved; pipe-live-unrecorded;
pty-live-unrecorded; success; child-failure; cancellation; lease-expiry; output-limit;
blocked-reader; disconnected-reader; persistence-error; restart-recovery; independent-accounting;
capacity-recovery; positive-control-sink-scan. Synthetic terminal canaries must be received live and
absent from SQLite, live WAL, operation/event payloads and diagnostic logs before cleanup/checkpoint.
A recording-enabled control must make the identical sink scan detect the canary. Portable tests do
not substitute for the delegated confinement lane.
