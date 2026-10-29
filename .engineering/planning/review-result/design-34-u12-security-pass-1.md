---
format: aep.planning-md/3
id: review-result:design-34-u12-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U12, the release preflight, pass 1 (2 warnings, 3 notes)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U12, the release preflight, pass 1

Reviewed `wave/k8s-namespace-1` at `3049fc17` (U12 unit commit `7da1a847`). Red cases: `7d91b1af` on
`sec/u12-release-preflight` (`xtask/tests/release_workflow.rs`, +217, test file only). Cases 88
before, 93 after, 4 red.

Verdict: NEEDS-CHANGE. No blocker, 2 warnings, 3 notes.

## Findings

```findings
- {file: .github/workflows/release.yml, line: 726, category: contract-drift, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "the preflight never requires a successful namespace-lane.yml run for the tagged commit, which design 34 section 10.4 requires and no other check covers"}
- {file: .github/workflows/release.yml, line: 1508, category: integrity, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "the executor handshake runs the image entrypoint, not /substrate-executor, the path the driver runs, so a differing entrypoint passes with an unchecked binary"}
- {file: .github/workflows/release.yml, line: 751, category: boundary, severity: note, verdict: CONFIRMED, origin: introduced, message: "a second empty Namespace-lane line after the record is not counted, so exactly one record is not enforced"}
- {file: .github/workflows/release.yml, line: 1529, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "executor binary refusals run after the daemon and MCP tags are pushed, contrary to the workflow's statement that every binary refusal precedes any tag move"}
- {file: .github/workflows/release.yml, line: 6, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "the header comment lists three images and omits the executor image and the namespace-lane refusal"}
```

## Red cases

- `security_review_the_release_requires_a_green_namespace_lane_workflow_for_the_tagged_commit`
- `security_review_the_executor_handshake_is_answered_by_the_binary_the_driver_runs`
- `security_review_a_tag_with_a_second_empty_namespace_lane_line_is_refused`
- `security_review_the_executor_binary_refusals_run_before_any_image_tag_moves`

## Cleared

- Every precondition is in `preflight`; `release` waits for it.
- Write-once executor tag, sign by digest, `cosign verify` with the exact workflow identity and an
  anonymous read-back, all before `gh release create`.
- The static check runs on the pushed image bytes; recovery reuse requires the revision label.
- Permissions unchanged, no new secret, no `pull_request_target`; tag messages and dispatch inputs
  reach `run:` through env vars only.
- The Namespace-lane record resists carrier and shape substitutions (green case).

## Not verified

Real GHCR, cosign or docker behaviour; the digest race between `docker push` and `imagetools
inspect`, shared with the daemon and MCP steps.

## Disposition

All five go to fix unit L. Finding 1: D182 moved the cluster run to a workstation record and kept
the static CI job, so the release also requires that job's success for the tagged commit, read
like `gate.yml`'s recorded conclusion. Finding 3 is fixed for every lane record step, not only
Namespace-lane. Finding 5: the stale parts of the header comment are deleted; nothing is added.
