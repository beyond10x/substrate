---
format: aep.planning-md/3
id: story:concurrent-snapshots-never-publish-an-older-probe
kind: story
status: proposed
title: Concurrent driver snapshots never publish an older probe result over a newer one
relations:
- informed_by: review-result:wave-2026-11-01-d354-u1-adversary-pass-1
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
---
# Story: concurrent driver snapshots never publish an older probe result over a newer one

## Outcome

Two `machine()` calls that interleave in the Kubernetes namespace driver leave the newest probe's
snapshot, generation and node published, never an older tuple over a newer one.

## Observation

Wave D354 adversary pass on U1 (`review-result:wave-2026-11-01-d354-u1-adversary-pass-1`, origin
pre-existing): `crates/substrate-kubernetes/src/lib.rs:96-101` computes `capability_snapshot()` under
the probe lock, releases it, then publishes under the exec lock. Two interleaving calls can publish
the older tuple. The same mechanism exists for snapshot and generation since before D354. No caller
was shown to reach the window; inferred from the code, not observed.

## Acceptance

- A case that interleaves two `machine()` calls (the older computed first, published last) ends
  with the newer probe's tuple published.
- An exec start after it compares against the newer tuple.
