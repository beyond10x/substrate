---
format: aep.planning-md/3
id: story:pod-sweep-replays-from-the-policys-create
kind: story
status: proposed
title: The sweep's replay starts from the policy's create
relations:
- decomposes: epic:confinement-contract-family
- informed_by: approval-record:d339-substrate-wave-2026-11-01
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T02:22:37Z", actor: "human:timo", revision: 2}
---
# Story: the sweep's replay starts from the policy's create

## Observation

The D339 adversary pass on `story:pod-orphaned-policy-is-swept` (out of scope there): the
never-created rule replays the run's Pod from the policy's `resourceVersion`, which is the policy's
last write, not its create. The adapter never writes a policy after creating it, but another writer
(an operator adding a label or an annotation) after the Pod was created moves the replay's start
past the Pod's whole history. The replay then shows no event of the name, and the policy of a
force-deleted Pod whose container may still run is swept. Design 28 § *A gone Pod* states the
assumption that the adapter is the only writer of its policies.

## Acceptance

- A policy whose metadata shows a write after its create (another `managedFields` entry, or its
  one entry's time after `creationTimestamp`) is not swept by the never-created rule; it is kept
  and logged, as a policy whose container is not shown stopped is.
- A policy unchanged since its create is swept as before.
- A fake-API case holds both and runs once in seconds.

## Not known

Whether the sweep's metadata-only list (`PartialObjectMetadata`) carries `managedFields` on the
served Kubernetes versions, or the rule needs a whole read of each candidate.
