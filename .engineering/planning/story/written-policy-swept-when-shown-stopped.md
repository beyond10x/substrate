---
format: aep.planning-md/3
id: story:written-policy-swept-when-shown-stopped
kind: story
status: implemented
title: A policy written after its create is still swept when its run's container is shown stopped
relations:
- informed_by: review-result:wave-2026-11-01-d354-u4-adversary-pass-1
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/pod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod_cleanup_never_created.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/gone.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T23:08:45Z", actor: "human:timo", revision: 6}
- {from: "active", to: "implemented", at: "2026-11-02T01:24:57Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
# Story: a policy written after its create is still swept when its run's container is shown stopped

## Outcome

The `pod` adapter's sweep removes a `NetworkPolicy` whose `managedFields` show a write after its
create when a replay shows its run's container terminated, as it did before D354; only the
never-created rule is withheld for such a policy.

## Observation

Wave D354 adversary pass on U4 (`review-result:wave-2026-11-01-d354-u4-adversary-pass-1`, finding 1):
`crates/substrate-host/src/confinement/pod.rs:571` starts the trace of a written policy at no version,
which turns off the stopped rule as well as the never-created rule. An uncommitted probe
(`Watch::Terminated`, two `managedFields` entries) failed with "PROBE kept"
(`pod_cleanup_never_created.rs:458`, exit 101). The cost is a leaked policy, the safe direction.

## Acceptance

- A written policy whose replay shows its run's container terminated is swept as stopped.
- A written policy whose replay ends in a `BOOKMARK` with no event of its Pod is still kept.

## Scope

Derived 2026-11-02 by `story-scoper` (wave D364). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong). Implemented in one unit with `story:kept-written-policy-log-names-the-write`: both edit the same functions.

- **Crate:** `b10x-substrate-host` (`crates/substrate-host`), cited. `substrate-kubernetes-client`, inferred, only if the fix goes into `PodTrace`.
- **Source:** `crates/substrate-host/src/confinement/pod.rs`, cited: `sweep_page` at 537, `since` at 565-575, the `None` at 571; the `sweep` never-created match at 476-478 and the `resweep` match at 731-734. That both need the written rule, inferred.
- **Source:** `crates/substrate-kubernetes-client/src/gone.rs` `Api::pod_history` at 292-299, cited: with no version it returns Unproven before any replay runs.
- **Symbols:** `sweep_page`, `sweep`, `resweep`, `written_after_create`, `PodTrace::since`, `Api::pod_history`, `Deferred` (pod.rs:629), cited as names; the edit set is inferred.
- **Tests:** `crates/substrate-host/src/confinement/pod_cleanup_never_created.rs` 233-257 (`a_policy_written_after_its_create_is_not_swept_as_never_created`) and helper `written` at 204, cited; they drive only `Watch::NeverCreated`. No written case with `Watch::Terminated` (`pod_cleanup_tests.rs:76`) exists, cited.
- **Story cite:** `pod_cleanup_never_created.rs:458` does not resolve (the file is 450 lines; the probe was uncommitted), cited.
- **Would collide with:** any unit touching `pod.rs` sweep/resweep or `PodTrace` (`PodTrace::since` is also called from `crates/substrate-kubernetes/src/exec.rs:180,676` and `pod.rs:1313,1318`), cited.
- **Confidence:** high on locations; medium on the fix's shape, inferred.
