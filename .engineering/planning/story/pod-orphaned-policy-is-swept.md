---
format: aep.planning-md/3
id: story:pod-orphaned-policy-is-swept
kind: story
status: draft
title: A policy whose Pod never existed is swept
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# Story: a policy whose Pod never existed is swept

## Observation

Unit 4 of `wave/adapter-io` (`story:pod-force-deleted-keeps-its-policy`, decision D173) releases a
run's NetworkPolicy after a Pod 404 only when a Pod read or a bounded watch replay shows the
container `terminated`. A policy whose Pod was never created (the adapter stopped between creating
the policy and creating the Pod) has no such event, so the open sweep now keeps it. The manual case
`adversary_lane_open_sweeps_an_orphaned_policy` in `crates/substrate-host/tests/adversary_pod.rs`
(run only with `SUBSTRATE_POD_MANUAL_CLEANUP=1`) expects it swept and is expected to fail.

## Acceptance

A policy is swept when a complete replay from the policy's resourceVersion shows no Pod of that
run name was ever added, and kept when the replay is incomplete (410, timeout or a watch that cannot
say the replay ended). A fake-API case holds both and runs once in seconds; the manual case passes
on a disposable kind cluster.

## Not known

Whether the served Kubernetes versions mark the end of a replay (watch bookmarks or
`sendInitialEvents`) with the Role's verbs alone.
