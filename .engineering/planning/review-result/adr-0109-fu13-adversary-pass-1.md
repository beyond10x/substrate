---
format: aep.planning-md/3
id: review-result:adr-0109-fu13-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: the CI pod lane runs the pod adversary and security cases'
relations:
- reviews: story:pod-lane-runs-adversary-and-security
revision: 1
---
# Adversary pass 1: the CI pod lane runs the pod adversary and security cases

Verdict: needs change. Read at unit `b121f4dc` (merged as `5ae8a8ab`) on tree `9d776eb9`; cases on
branch `review/adr-0109-fu-pod-adversary` at `61595726`. Executed 28 to 34, 5 red.

## Findings

- A1 `scripts/pod-lane.sh:52` (warning, introduced): the guard required one passed case per suite,
  which offline cases always supply, so a run with every cluster case filtered out or ignored went
  green. Red: three cases in `xtask/tests/pod_lane_script_adversary_pass1.rs`.
- A2 `pod-lane.sh:42` (note, introduced): `printf | grep -q ABSENT` under `pipefail` reads a match
  as no match when more output follows than a pipe holds.
- A3 `crates/substrate-host/src/confinement/pod.rs:1302` (warning, pre-existing): a run read
  `Unknown` keeps its NetworkPolicy after the process holding the adapter ends.
- A4 `pod.rs:1325` (warning, pre-existing): after `Unknown` the adapter stops after two removal
  rounds, so a kubelet absence of about two minutes leaves the policy.
- A5 `crates/substrate-host/tests/adversary_pod.rs:525` (warning, introduced): the echo case run
  right after the kubelet-stop kill case read `exit: None` in 1 of 2 local runs.
- A6 `crates/substrate-host/tests/security_review_pod.rs` (note, introduced): the suite reads no
  lane variable, so running it in the pod lane added no cluster coverage.

## Outcomes

- A1 fixed in `20396932`: the script refuses any argument and requires `0 failed; 0 ignored; 0
  measured; 0 filtered out` per suite; the three cases pass (`ok. 4 passed`).
- A2 fixed in `20396932` with a here-string; the same defect in `scripts/delegated-lane.sh` fixed
  in `5677d620`.
- A3 and A4: not fixed in this release; recorded as `story:pod-unknown-cleanup-outlives-adapter`.
  Their cases in `adversary_pod_cleanup_pass1.rs` take 77 s and 158 s, name that story and run by
  hand against kind, outside the lane.
- A5 fixed in `20396932`: the kill case restores the node through a guard that waits for Ready with
  no `node.kubernetes.io` taint. Cause inferred from an observed 1 to 4 s taint lag, not reproduced.
- A6 fixed in `20396932`: `security_review_pod` leaves the lane and stays in the workspace gate.
- Gate run 40482047612 on the unit: Pod lane success in 5m11s. Merged on the follow-up branch as
  `1255aafd`.
- No separate security review: the unit changes CI scripts and tests, not confinement code; this
  pass checked the no-log ServiceAccount and token masking, which held.
