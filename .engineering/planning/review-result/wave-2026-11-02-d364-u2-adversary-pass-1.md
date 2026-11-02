---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u2-adversary-pass-1
kind: review-result
status: active
title: 'D364 U2 adversary pass 1: access review evaluation error'
relations:
- reviews: story:access-review-evaluation-error-is-not-a-denial
revision: 1
---
# Adversary pass 1 — U2 of wave D364

Unit `story:access-review-evaluation-error-is-not-a-denial`, tree `substrate-d364-u2` at `a2bece7b`
(merged into `wave/2026-11-02-d364` at `26597e3c`). Report as returned by the adversary
(`aep:adversary`, Opus); machine paths shortened to the unit's scratch.

```
unit: story:access-review-evaluation-error-is-not-a-denial, tree substrate-d364-u2 at a2bece7b plus 2 uncommitted test files
verdict: blocked
cases: executed n/a→1 (one case alone; the suite was refused), red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 2 paths (the unit's adversary scratch)
needs-coordinator: yes (re-run the targeted suites when the slot frees, then commit the green cases)
```

Blocked: the pre-check refused the targeted suite run (`CHECK-FAILED 2 other cargo`, exit 3).
Nothing is committed; all four cases are uncommitted in the tree. One is red, and three are expected
green but have never run.

Cases added (test files only, no implementation file touched; `client_tests.rs` +57,
`namespace/verb_review_tests.rs` +103):

- `verb_review_tests.rs:480` `d364_adv::a_denied_create_is_still_named_when_the_get_review_does_not_decide`:
  red. The Role holds only `get`, so the `create` review is a plain RBAC denial with no error, and
  the `get` review answers `evaluationError`. The case asserts that the refusal still contains
  `a SelfSubjectAccessReview denies pods/exec create`. Run alone, exit 101:
  `the create review answered allowed: false with no error, and the refusal does not name create: the cluster API: exec answered 403: pods "sbx-pr-gbyb7pajquihb757-keeper" is forbidden: (a SelfSubjectAccessReview of pods/exec get does not decide the refused verb: its evaluationError is webhook authorizer: connection refused)`;
  `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 164 filtered out`.
- `verb_review_tests.rs:504` `a_denial_beside_an_evaluation_error_is_the_plain_denial_the_lane_matches`:
  `denied: true` beside an error gives exactly the S6 line after 2 reviews. Not run.
- `verb_review_tests.rs:524` `an_evaluation_error_echoing_the_token_reaches_the_refusal_cleared_and_bounded`:
  the token is redacted and the text bounded to 300 characters, the 403 kept as the prefix, 1 review
  asked. Not run.
- `client_tests.rs:2318` `d364_adv::an_access_review_reads_the_remaining_status_shapes`: an explicit
  `null` error reads `Ok(false)`; `denied: false` or `denied: "true"` beside an error reads
  `Err(error)`; a newline survives as text; an error with no `allowed` reads `ApiError::Invalid`.
  Not run.

Finding, `crates/substrate-kubernetes/src/namespace/workspace.rs:631-636`:

- What was measured: when `create` is denied with no error and then `get` answers an
  `evaluationError`, the `create` denial already collected is thrown away. The refusal does not name
  `create`, which contradicts Acceptance bullet 2 ("a review answering `allowed: false` with no error
  still names the denied verb").
- Design 34: the amendment (`docs/design/34-kubernetes-namespace-driver.md:895`) says "is not named
  alone", which a reader takes to mean `create` is still named. The unit's own test
  (`verb_review_tests.rs:~160`, the `before == 1` row) asserts that `create` is dropped.
- Fix (not applied): on `Ok(Err)` after earlier denials, keep `{ACCESS_REVIEW_DENIES}<denied>` and
  append that `<verb>` does not decide.
- What reaches it: nothing found. It needs an authorizer that answers the `create` review with no
  error and the `get` review with an error; the lane is RBAC only, and the story says evaluation
  errors are inferred, not observed. Verdict INFEASIBLE.

Attacked and not broken: `create` undecided stops the asking after 1 review; a plain denial is
still the exact S6 text and the xtask matches it by `trim() ==`; `allowed: true` or `denied: true`
beside an error decides; an empty error is no error; a non-string error is Invalid, so the verb is
unknown; the token is cleared before the 300-character bound; the prefix moved from
`refused_handshake` into `denied_exec_verbs` keeps the existing unknown-verb texts; mutants on
`allowed ||`, `denied ||`, `is_empty()` and `Value::Null` are each caught by the unit's client case;
a whitespace-only error reads as undecided (defensible); control characters pass through `quote`
verbatim, as every quoted API text does (outside the Acceptance).

```findings
[{"file": "crates/substrate-kubernetes/src/namespace/workspace.rs", "line": 631, "category": "acceptance", "severity": "warning", "verdict": "INFEASIBLE", "origin": "introduced", "message": "scope in; case d364_adv::a_denied_create_is_still_named_when_the_get_review_does_not_decide: a create review denied with no error is dropped from the refusal when the following get review answers an evaluationError, against acceptance bullet 2 and the design 34 amendment's reading, reached by no lane or configuration found"}]
```
