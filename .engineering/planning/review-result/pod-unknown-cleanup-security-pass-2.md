---
format: aep.planning-md/3
id: review-result:pod-unknown-cleanup-security-pass-2
kind: review-result
status: active
title: Security review of the Pod sweep empty-list fix, pass 2
relations:
- reviews: story:pod-unknown-cleanup-outlives-adapter
revision: 1
---
# Security review of the Pod sweep empty-list fix (PR 184), pass 2

Reviewed commit `d54ff038` on `fix/pod-unknown-cleanup-leftovers`, 2026-10-28: `list_items` reads an
empty `PartialObjectMetadataList` whose `items` is null or absent as empty.

No findings. A list only decides which Pods are read; every policy delete still needs a read of
that exact Pod answering 404 (`api.get` returning `Ok(None)`), so a null, absent or cut-short list
can lose an orphan for a round but cannot delete a policy. A 200 `Status` body is refused by the
kind check in both sweeps. The open sweep's age floor reads the API server's `Date` and is
unchanged.

Cases added, both green:

- `pass2_a_short_or_null_pod_list_never_deletes_a_policy_whose_pod_answers`: the deferred sweep
  against null, absent and cut-short Pod lists keeps every policy whose Pod still answers, deletes
  only the one whose Pod answers 404, and a `Status` body deletes nothing;
- `pass2_the_open_sweep_on_a_short_page_keeps_young_and_present_policies`: the open sweep on a
  cut-short page deletes only the old orphan whose Pod answers 404.
