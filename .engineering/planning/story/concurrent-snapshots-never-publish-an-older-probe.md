---
format: aep.planning-md/3
id: story:concurrent-snapshots-never-publish-an-older-probe
kind: story
status: active
title: Concurrent driver snapshots never publish an older probe result over a newer one
relations:
- informed_by: review-result:wave-2026-11-01-d354-u1-adversary-pass-1
scope:
- confidence: inferred
  path: crates/substrate-kubernetes/src/exec_tests.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/lib.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/namespace/probe.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/namespace/probe_tests.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T23:08:45Z", actor: "human:timo", revision: 6}
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

## Scope

Derived 2026-11-02 by `story-scoper` (wave D364). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-kubernetes` (the namespace driver), cited: the story names `crates/substrate-kubernetes/src/lib.rs:96-101`.
- **Race site:** `crates/substrate-kubernetes/src/lib.rs:95-103`, `fn machine()` computes the snapshot, then publishes it, cited.
- **Probe lock:** `crates/substrate-kubernetes/src/namespace/probe.rs:423-441` (`capability_snapshot`, takes `lock(&self.probe)` at :433); `ProbeState` at :226; writes at :306 and :355, cited.
- **Exec lock:** `crates/substrate-kubernetes/src/exec.rs:138-141` (`publish_exec_snapshot`), `:71` (`Execs::published`), `:298` (`admitted_generation`, the start's comparison), cited.
- **Symbols:** `Driver::machine`, `NamespaceDriver::capability_snapshot`, `NamespaceDriver::publish_exec_snapshot`, `ProbeState`, `Execs::published`, `NamespaceDriver::probe` (`namespace.rs:94`), `execs()` (`namespace.rs:589`), cited.
- **Existing tests:** `probe_tests.rs` `rig.machine()` (rig at :378; `the_snapshot_is_published_with_the_probes_node_and_a_later_probe_moves_it` at :2502); `exec_tests.rs` `d354_adv::a_stale_snapshot_is_capability_stale_before_the_node_is_compared` at :2168, cited.
- **Test file for a new case:** `crates/substrate-kubernetes/src/namespace/probe_tests.rs` (module `d354_adv`, :2604); start-side assertion in `crates/substrate-kubernetes/src/exec_tests.rs` (module `d354_adv`, :2114), inferred.
- **Thread precedent:** `crates/substrate-kubernetes/src/driver_tests.rs:1312` uses `std::thread::scope`, cited.
- **Documents:** none.
- **Confidence:** medium. The defect and lock sites are cited; `machine()` has no await, so a deterministic interleave probably needs a test seam (inferred).
- **Would collide with:** any unit touching `machine()` in `lib.rs`, `capability_snapshot`/`ProbeState` in `probe.rs`, or `publish_exec_snapshot`/`admitted_generation` in `exec.rs`.
- **Safety fact:** the only production caller of `publish_exec_snapshot` is `lib.rs:97`, so `machine()` is the sole writer of the tuple (repo-wide `git grep`).
