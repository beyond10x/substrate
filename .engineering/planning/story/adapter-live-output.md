---
format: aep.planning-md/3
id: story:adapter-live-output
kind: story
status: draft
title: A confinement adapter delivers a child's output while it runs
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:adapter-live-spec
- depends_on: story:adapter-stdin-pipe
revision: 3
---
# A confinement adapter delivers a run's output while it runs

## Design

A consumer needs a child's output while it runs; today it is read after the run ends.
[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 3, § 5, § 11 story 4, D3
to D6, D12 (the `Live` refusal), D16: `OutputDelivery::Live { queued_chunks }`, `OutputEvent`,
`read_output` under a per-run reader lock, `output_stalled`, the live queue in the shared drain and
in `drain_capped`'s adapter path, and `output_live` on `local`, `linux`, `systemd` around both,
`macos` and `oci`.

## Depends on

`story:adapter-live-spec` (lands with it as one unit) and `story:adapter-stdin-pipe` (it extends
that story's `check_streams`, `run_with_stdin`, linux `Pipes` channel and `systemd` forwarding).
Edits no scenario and no pinned scenario count.

## Acceptance

- On a natural end each stream's chunks are in order with contiguous offsets, the record equals
  them with no marker, and crossing the bound is `Bounded` from the delivered offset and the final
  `*_truncated`, then `None` (T4, T6).
- An unread run stalls after 1 s, ends, and reads `Stalled` after its queued chunks;
  `output_stalled` is true (T5, T11). On any other imposed end delivery stops, no event follows,
  and the chunks are a prefix of the record (T19).
- `check_streams` refuses `queued_chunks` 0 and 17 and `Live` with `Tail` (T1b);
  `run_with_stdin` refuses `Live` `confinement.live-output-unread` before start (T7b).
- `pod` refuses `Live` naming `output_live` (T17); `systemd` around `linux` forwards `read_output`
  and `output_stalled` (T13).
- Every lane is green on the unit's head; macos cases move `macos.yml`'s pinned pass count.
