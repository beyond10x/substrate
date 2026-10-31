---
format: aep.planning-md/3
id: story:clean-room-destroy-settles-on-the-namespace-driver
kind: story
status: implemented
title: The clean-room runner's destroy and commit-order cases settle on the namespace driver
relations:
- informed_by: story:remote-clean-room-conformance
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T23:39:43Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-30T23:39:43Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-31T02:22:51Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: The clean-room runner's destroy and commit-order cases settle on the namespace driver

## Outcome

Step 4 of `cargo xtask namespace-lane observe` (design 31's clean-room runner against the
`--driver kubernetes` daemon over TLS) passes `workspace.destroy` and `events.commit-order`, and no
workspace the runner created outlives the step, so steps 5 and 6 start in a namespace under its
ResourceQuota.

## Finding

The lane at `28bf24fe` (after PR 199 let step 4 start): the runner's verdict was failed, with
`workspace.destroy (operation.outcome-unknown, refused)` and
`events.commit-order (operation.outcome-unknown, refused)`, 34 passed, 17 absent. Step 5
(`namespace_lane_drives_the_shipped_daemon_over_the_kubernetes_driver`,
`crates/substrate-daemon/tests/runtime_vectors.rs:7420`) and both step 6 cases then got 429
`workspace.capacity`: the namespace's quota admits four PersistentVolumeClaims. The cause of
`operation.outcome-unknown` is not yet known.

## Acceptance

- The cause is found by a red-capable feedback loop and recorded (the `aep:diagnosing` method),
  with evidence; if none is found, the evidence is recorded and nothing is guessed.
- The fix makes both cases pass against the namespace driver, or, if the runner's expectation is
  what is wrong, changes the runner with the design 31 reference that says so.
- Step 4 leaves no workspace of its own behind, whatever its verdict, before step 5 starts.
- A red case first where a test can hold it (fake API or the runner's own suites); the lane run
  itself is the end-to-end check.
