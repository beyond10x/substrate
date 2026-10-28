---
format: aep.planning-md/3
id: review-result:pod-unknown-cleanup-security-pass-1
kind: review-result
status: active
title: Security review of the Pod Unknown cleanup fix, pass 1
relations:
- reviews: story:pod-unknown-cleanup-outlives-adapter
revision: 1
---
# Security review of the Pod Unknown cleanup fix (PR 184)

Reviewed `fix/pod-unknown-cleanup-leftovers` at `b13033b7` (base `5b9daca9`), 2026-10-28.

No path was found that removes a run's NetworkPolicy while its Pod can still run.

| id | severity | where | finding | outcome |
|---|---|---|---|---|
| F1 | S3, introduced | `crates/substrate-host/src/confinement/pod.rs` `owned_pods` | The deferred sweep listed whole Pod objects, 100 per page, under a 4 MiB page bound. About nine live runs at the adapter's own argv and environment bounds made every round fail, so no kept policy was removed after its Pod was gone. Red case `review_ten_admitted_runs_alive_do_not_stall_the_deferred_sweep`. | fixed: both sweep lists request `PartialObjectMetadataList`; the case is green, and red again with the old `Accept` header |
| J1 | S4, introduced | `pod.rs` deferred thread | Every adapter whose open kept a live run listed all owned Pods as whole objects every 2 s. | fixed with F1: the list carries metadata only |
| J2 | S4, pre-existing | `pod.rs` Pod read | Any 404 counts as gone and the policy is deleted by name without a uid precondition, so a force-deleted Pod whose container still runs on a lost node loses its policy. No path inside the adapter reaches it. | not fixed here; open |

Checked and cleared: the S1 rule of PR 176 (a young policy whose Pod was never seen stays until 90 s by the API server's `Date`), the seen marking (only a 200 for the exact name), API errors (a failed or truncated list deletes nothing), namespace scoping, the deferred thread's lifetime (`Weak` reference, ends one round after the adapter), and the request bound of the open sweep.

Cases added by the review: `review_the_deferred_sweep_deletes_only_after_a_pod_read_answers_404` (green) and `review_ten_admitted_runs_alive_do_not_stall_the_deferred_sweep` (red, then green), in `crates/substrate-host/src/confinement/pod_cleanup_review.rs`.
