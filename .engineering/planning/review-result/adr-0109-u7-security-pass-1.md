---
format: aep.planning-md/3
id: review-result:adr-0109-u7-security-pass-1
kind: review-result
status: active
title: Security review pass 1 on story:pod-adapter
relations:
- reviews: story:pod-adapter
revision: 1
---
# Security review pass 1: story:pod-adapter

Verdict: needs change. Read at the unit head 3312ca3 (integration 777e566); 7 cases added in
`crates/substrate-host/tests/security_review_pod.rs`, 2 red; case 2 reproduced on kind with the
gate's pinned node image.

## Findings

- F1 `crates/substrate-host/src/confinement/pod.rs:1184` (warning): a Pod create with an ambiguous
  result (dropped connection, 504, client timeout) deletes the NetworkPolicy while the stored Pod
  runs untracked without it, against design 28's created-first, deleted-last rule.
- F2 `pod.rs:883` (warning): argv and env values are rendered unescaped, so the kubelet's `$(VAR)`
  and `$$` expansion runs a different argv than requested and `run_started` names a program that
  did not run.
- F3 `pod.rs:1130` (note, infeasible today): the monitor deletes the policy when the Pod removal
  was not confirmed.
- F4 `pod.rs:878` (note): the workspace `emptyDir` has no `sizeLimit` and the container no
  `ephemeral-storage` limit.

## Verified to hold

TLS verified against the kubeconfig's CA data only, plain http and skip-verify refused; 14
kubeconfig members refused by name with 0 requests; no credential in Display or Debug; credentials
and namespace only from `PodRequest.cluster`; the posture design 28 states; default-deny policy;
deletes name only the run's own objects; `pod-lane` job pins actions by SHA, the kind binary by
sha256 and the node image by digest, `contents: read`, no secrets.

## Decisions taken by the coordinator

F1 and F3: on an ambiguous create or an unconfirmed removal the adapter deletes the Pod by name and
confirms it is gone before it deletes the policy; if it cannot confirm, the policy stays and the run
is reported `Unknown`. F2: every `$` in command, args and env values renders as `$$`; the golden
manifest is updated. F4: the `emptyDir` gets a `sizeLimit` and the container an `ephemeral-storage`
limit, both 1 GiB unless the operator's pod settings state another value.
