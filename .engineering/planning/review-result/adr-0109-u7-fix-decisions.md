---
format: aep.planning-md/3
id: review-result:adr-0109-u7-fix-decisions
kind: review-result
status: active
title: 'Pod review fixes: decisions the review records did not state'
relations:
- reviews: story:pod-adapter
revision: 1
---
# Pod review fixes: decisions the review records did not state

The fix for `review-result:adr-0109-u7-security-pass-1` and `-u7-adversary-pass-1` (commit
`dd43c91`, merged as `dcd9bd0`) took these decisions. The coordinator accepts each one.

| # | Decision | Reason |
|---|---|---|
| P1 | The `pods/log` check at open GETs the log of the absent Pod `substrate-log-access-probe`. 200, 400 or 404 admits the credential; 401, 403 or any other answer is `confinement.adapter-unserved`. | The check stays inside the namespace; no SelfSubjectAccessReview. |
| P2 | With `pod_pids_limit` declared and no processes bound in the profile, the run is admitted under that limit. A profile bound below the limit is refused. | The operator's limit is the bound. |
| P3 | One extension member, `ephemeral_storage_bytes` (1 MiB to 1 TiB), sets both the emptyDir `sizeLimit` and the container ephemeral-storage request and limit. | One bound, one setting. |
| P4 | No Pod is force-deleted. A Pod gone after the adapter's own graceful delete counts as a confirmed stop. | The kubelet confirms the stop. |
| P5 | Kill uses a 1 s grace, Term 5 s. After a stop request, confirmation is awaited for 60 s (`STOP_CONFIRM`); after that the run reads Unknown. | A grace of 0 is a force delete. |
| P6 | When a create answer is lost and the Pod's removal is not confirmed, start fails `confinement.pod-api-failed`, naming the Pod. The policy stays. | No run handle for a Pod the adapter cannot see. |
| P7 | A NetworkPolicy left by a failed policy create is removed. | Nothing is left behind. |
| P8 | A 400 log answer before the container started reads as empty output. Every other unreadable log is a named error. | Before the start, nothing has been written. |
| P9 | The ess deadline lane case asserts a non-zero exit instead of `DeadlineExceeded`. | `termination()` was removed; the observed run ended 137. |
| P10 | The CI `pod-lane` job still runs only `ess_confinement_pod`. `adversary_pod` and `security_review_pod` lane cases ran on a local kind cluster: 7 and 7 passed, `ess_confinement_pod` 14 passed. | They need a no-log ServiceAccount; wiring them into CI is a separate change. |
