---
format: aep.planning-md/3
id: story:pod-unknown-cleanup-outlives-adapter
kind: story
status: draft
title: A pod run read Unknown leaves no NetworkPolicy behind
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:pod-adapter
revision: 2
---
# A run read Unknown leaves no NetworkPolicy behind

## Outcome

When a pod run's status reads `Unknown` (kubelet unreachable after a kill), its NetworkPolicy is
removed once the Pod is gone, even if the process holding the adapter has exited or the kubelet
stays away longer than two removal rounds.

## Found by

Adversary pass 1 on `story:pod-lane-runs-adversary-and-security` (branch
`review/adr-0109-fu-pod-adversary` at `61595726`), findings A3 and A4, both pre-existing in
`crates/substrate-host/src/confinement/pod.rs`:

- A3 (`pod.rs:1302`): cleanup after `Unknown` lives only in a background task of the process that
  holds the adapter; if that process ends, the NetworkPolicy stays and no open sweeps it.
- A4 (`pod.rs:1325`): after `Unknown` the adapter makes two rounds of `REMOVE_ATTEMPTS` (about
  31 s each) and stops, so a kubelet absence of about two minutes leaves the policy with the
  adapter alive.

Red cases: `crates/substrate-host/tests/adversary_pod.rs`, module `cleanup_pass1` (runs only with `SUBSTRATE_POD_MANUAL_CLEANUP=1`)
(`adversary_lane_a_run_read_unknown_leaves_nothing_once_its_process_has_ended`, 77 s;
`adversary_lane_a_run_read_unknown_is_removed_after_a_longer_kubelet_absence`, 158 s). Both wait
on the adapter's fixed 60 s `STOP_CONFIRM`, so they run manually against a kind cluster and are
not in `scripts/pod-lane.sh`.

## Acceptance

- Open sweeps the namespace for labelled NetworkPolicies whose Pod no longer exists.
- After `Unknown`, removal retries until the Pod is gone.
- Both red cases above pass on a kind cluster.
- A case that finishes in seconds (fake API server) holds the sweep in the workspace gate.
