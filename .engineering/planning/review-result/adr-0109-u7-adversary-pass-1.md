---
format: aep.planning-md/3
id: review-result:adr-0109-u7-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:pod-adapter
relations:
- reviews: story:pod-adapter
revision: 1
---
# Adversary pass 1: story:pod-adapter

Verdict: needs change. Read at the unit head 3312ca3 (integration 777e566); 7 cases added in
`crates/substrate-host/tests/adversary_pod.rs`, 4 red portable and 7 red with the lane on kind.

## Findings

1. `crates/substrate-host/src/confinement/pod.rs:883` (blocker): command, args and env values are
   rendered unescaped; the kubelet rewrites `$$` and `$(NAME)` (seen on kind). Same defect as
   security F2.
2. `pod.rs:1184` (warning): a Pod create whose answer is lost after commit leaves an untracked Pod
   running while start deletes its policy. Same defect as security F1.
3. `pod.rs:530` (blocker): an unreadable log (403 without `pods/log`, which the open probe does not
   check, or any transient failure) is reported as complete, untruncated, empty output.
4. `pod.rs:1129` (warning): with the API server silent, wait and observe stay Running past 300 s
   because the phase is sent only after the removes, against `UNREACHABLE_LIMIT`.
5. `pod.rs:1261` (warning): Kill and the deadline give-up force-delete with grace 0, so wait
   reports Ended by SIGKILL while the container still runs on a node whose kubelet is silent.
6. `pod.rs:112` (warning): `memory_oom_kills` is only on `PodAdapter::termination`, not on the wait
   observation the acceptance names; a client must branch on the driver (invariant 4).
7. `pod.rs:69` (note): without `pod_pids_limit` the default processes bound is never applied.

## Checked and not broken

Names, labels, namespace and workdir injection; the image pin; refused kubeconfig members; TLS
with only the operator's CA and no redirects; the golden posture; default-deny policy both ways;
bounded output; the lane fails with an absent kubeconfig or dead cluster; admission refusals audited.

## Decisions taken by the coordinator

1 and 2: as the security pass decided. 3: `open` checks `pods/log` access and is refused
`confinement.adapter-unserved` without it; a log read is retried, and a log that cannot be read
is a named error from `output_read`, never empty output. 4: the run reads `Unknown` once
`UNREACHABLE_LIMIT` passes, whatever the removes are doing. 5: a Kill or give-up reports Ended only
after the API shows the container terminated; otherwise `Unknown`. 6: `RunObservation` gains
`memory_oom_kills: Option<u64>` (the pod implementor's patch), `None` on every adapter that cannot
observe it. 7: with no declared pods PID limit, a profile's processes bound, the default included,
is refused `confinement.pod-processes-unserved`.
