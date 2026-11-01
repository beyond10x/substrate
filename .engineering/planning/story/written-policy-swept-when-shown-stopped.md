---
format: aep.planning-md/3
id: story:written-policy-swept-when-shown-stopped
kind: story
status: proposed
title: A policy written after its create is still swept when its run's container is shown stopped
relations:
- informed_by: review-result:wave-2026-11-01-d354-u4-adversary-pass-1
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
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
