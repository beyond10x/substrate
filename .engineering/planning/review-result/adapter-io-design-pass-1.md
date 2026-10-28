---
format: aep.planning-md/3
id: review-result:adapter-io-design-pass-1
kind: review-result
status: active
title: Independent review of design 32, pass 1
relations:
- reviews: story:adapter-io-spec
revision: 1
---
# Independent design review of design 32, pass 1

Reviewed `docs/design/32-adapter-stdin-and-live-output.md` at PR 189 head d1d1deb3 with
`story:adapter-stdin-pipe` and `story:adapter-live-output`, 2026-10-28. Verdict: needs-revision.
Citations: process.rs is `crates/substrate-host/src/process.rs`; D32 is the design.

- F1 (blocking): "the record is what was delivered" (D32:183-188) is false twice. On `linux`,
  `drain_capped` overwrites the last 31 bytes of a truncated record with the marker
  (process.rs:3706-3711), and after a stall `drain_capped` keeps filling `kept` up to the bound
  (process.rs:3660-3670) while forwarding stopped. Pass `mark_truncation=false` with a live sender,
  as the pty path does (process.rs:3612-3632); state "record contains the delivered head, equal
  unless `Stalled`".
- F2 (blocking): no mechanism for `Bounded`, `Stalled`, `Interrupted` (D32:178-182, 231). The host
  queue carries only `PipeFrame{stream,bytes}` (observation.rs:13-16); a stall happens when the
  queue is full, so a queued `Stalled` has no slot. `read_output` should synthesize the events from
  drain flags after the queue is exhausted and track offsets itself.
- F3 (blocking): "cancellation closes" (D32:96-98) has no mechanism; `write_pipe`
  (process.rs:1446-1520) is not cancel-safe. A drop guard sets a synchronous poison flag checked
  under the stdin lock, in an adapter-side wrapper; test a write cancelled mid-chunk then a write
  answered `session.input-closed`.
- F4 (blocking): pending writes (D32:91-95): `close_pipe_input` takes the same stdin mutex
  (process.rs:1540), so `close_stdin` blocks behind a stuck write; `write_stdin` has no deadline;
  run end does not wake a write held by a descendant holding fd 0. State the semantics and test
  each.
- F5 (blocking): `run_with_stdin` on a `Live` profile never reads events (D32:104-107), so it
  stalls and is killed after 16 chunks. Refuse `Live` there by a named code, or drain; state that
  full duplex under `Live` needs a concurrent reader.
- F6 (blocking): the `oci` `stdin_pipe` fact is static while its safety is a lane result
  (D32:234, 366); detach keys could drop bytes on an unlisted runtime. Probe at open or declare
  only for pinned runtime and version pairs (invariant 3; atlas ADR 0109 decision 2).
- F7 (blocking): public API breaks understated (D32:221-223): `FACT_ORDER: [Fact;11]` and
  `FAMILY_ORDER` are public fixed-length arrays (facts.rs:13,28); `Fact` is serde
  (generated/confinement.rs:88); `ConfinementProfile` (profile.rs:40) gains two public fields; make
  `StdinMode` and `OutputDelivery` `#[non_exhaustive]` and list every break.
- F8 (blocking): the two stories cannot land independently and no edge says so; "lands with its
  ESS scenarios in the same change" conflicts with ADR 0044 decision 6 and D32 §6. Add the spec
  story and `depends_on` edges, or split scenarios per fact.
- F9 (note): stall threshold misdescribed (D32:189-193): chunks are 8 KiB (process.rs:3646) or
  16 KiB (output_retention.rs:224); state it per adapter or coalesce to 64 KiB; state memory over
  the 128 tracked runs (local.rs:50).
- F10 (note): Q7 (D32:369) fails in the stall case; add a non-breaking `output_stalled(run)` or
  record the loss under D4.
- F11 (note): `pod` Live labels the kubelet-merged stream `Stdout` (D32:235); document it with the
  fact or defer `pod`.
- F12 (note): missing one-scenario cases: cancel, close during a pending write, child exit during a
  write, stall with both streams active, stall on `systemd`; T5 should name its time bound.
- F13 (note): "no wire change" holds; "no ADR" holds except that D32 extends ADR 0031's 1 s kill
  to non-session embedders; say so in D4 or Q5.
