---
format: aep.planning-md/3
id: story:local-live-kill-case-waits-for-queued-chunk
kind: story
status: draft
title: The local live-kill adversary case waits for its chunk to be queued
relations:
- decomposes: epic:confinement-contract-family
scope:
- confidence: cited
  path: crates/substrate-host/tests/ess_confinement_local.rs
- confidence: cited
  path: crates/substrate-host/tests/support/mod.rs
revision: 3
---
# Story: The local live-kill adversary case waits for its chunk to be queued

## Outcome

`adv_local_live_kill_delivers_only_the_queued_chunk` (`crates/substrate-host/tests/ess_confinement_local.rs`,
helper in `crates/substrate-host/tests/support/mod.rs` near line 1321) asserts that a queue of one
holds one chunk at the Kill. It sends the Kill once a marker file appears, which does not prove the
adapter's reader has queued the chunk yet. On a loaded host the Kill can arrive first, and the case
reads 0 chunks.

## Finding

Full local gate on `wave/k8s-namespace-1` at `b1a6ee18`, 2026-10-29, host load average about 60 on
20 cores: `assertion left == right failed: local adv kill: a queue of one held one chunk at the
Kill; 0 bytes were delivered` (left 0, right 1). The same binary rerun at load 22 passed 44 of 44.
The code under test did not change in that wave.

## Acceptance

- The case sends the Kill only after an observation that the chunk is queued (an adapter-visible
  state or the first event read), not after a file the child writes.
- The assertion that exactly one chunk is delivered is unchanged.
- The case runs once, in seconds.

## Scheduling

Draft, for the next substrate wave.
