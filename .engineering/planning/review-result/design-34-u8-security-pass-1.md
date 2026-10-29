---
format: aep.planning-md/3
id: review-result:design-34-u8-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U8, the workspace path, pass 1 (1 warning, 1 note)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U8, the workspace path, pass 1

Reviewed `wave/k8s-namespace-1` at `08d940dc` (U8 unit commit `69bfe835`). Red cases: `d2a84f2e` on
`sec/u8-workspace-path` (`crates/substrate-kubernetes/src/namespace/workspace_tests.rs`, +191, test
file only). Lib suite 32 before, 37 after, 2 red.

Verdict: one warning, one note; no blocker.

## Findings

```findings
- {file: crates/substrate-kubernetes/src/namespace/files.rs, line: 209, category: integrity, severity: warning, verdict: CONFIRMED, origin: introduced, message: "a lost replace-v2 answer whose stat shows the requested bytes is reported as an applied compare-and-swap, although the executor refused it"}
- {file: crates/substrate-kubernetes/src/namespace/workspace.rs, line: 404, category: contract-drift, severity: note, verdict: INFEASIBLE, origin: introduced, message: "the quota match ignores the Status reason D184 names; no request-controlled text can reach the message, so it is not reachable in U8"}
```

## Red cases

- `sec_a_lost_answer_never_turns_a_refused_create_if_absent_into_success` (line 897).
- `sec_a_lost_answer_never_reports_a_compare_and_swap_from_a_digest_the_file_never_had` (line 920).

## Cleared

- Every create is preceded by its ledger rows; a lost create is resolved by one read and adoption.
- Every delete carries the bound uid as a precondition; a same-named or different-uid object is
  refused.
- An unbound row is never settled before the N9 bound.
- File calls run the fixed command `/substrate-executor fs` with the request on stdin; a path that
  leaves the volume is refused by the executor.
- `check_root` refuses near-miss names on every route and sends nothing.
- The keeper Pod meets `restricted`.
- No secret or token in errors or logs.

## Not verified

A real cluster; the window between the keeper ownership check and `pods/exec`, which addresses the
Pod by name.

## Disposition

Coordinator decision (inside design 34): a lost answer to a conditional write (`replace-v2` with a
create-if-absent or an expected `before_sha256`) is `Failed` `kubernetes.executor-unanswered`,
retriable, with no success inferred from the stat, as edit and patch already are; unconditional
writes keep the stat resolution. A retry of the conditional write is safe: if the first call
applied, the retry's precondition no longer holds and it is refused. Fix unit K implements it; U13
amends design 34 § 5's file row. The note stays: the client does not expose the Status reason.
