---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u2-adversary-pass-2
kind: review-result
status: active
title: 'D364 U2 adversary pass 2: the evaluation-error correction'
relations:
- reviews: story:access-review-evaluation-error-is-not-a-denial
revision: 1
---
# Adversary pass 2 — U2 of wave D364

Unit `story:access-review-evaluation-error-is-not-a-denial`, correction `a2bece7b...c34e8bd2`
(merged into `wave/2026-11-02-d364` at `3bb04dac`), tree `substrate-d364-u2` at `9dd0a30b` plus two
uncommitted test files. Report as returned by the adversary (`aep:adversary`, Opus); machine paths
shortened to the unit's scratch.

```
unit: story:access-review-evaluation-error-is-not-a-denial, correction a2bece7b...c34e8bd2, tree substrate-d364-u2 at 9dd0a30b plus 2 uncommitted test files
verdict: blocked (clippy refused by the pre-check: 2 other cargo; every test run green)
cases: executed 15→20 (kubernetes verb_review), 4→5 (client access_review), red 0
origin: introduced 0 / pre-existing 1 / undecided 0
wrote-outside-worktree: 10 log files in the unit's adversary scratch
needs-coordinator: yes (run clippy on the two crates, then commit the two green test files)
```

Cases added, each green on its first run alone:

- `verb_review_tests.rs`, `mod d364_adv2`:
  `a_denied_create_before_an_undecided_get_keeps_the_error_cleared_and_bounded`,
  `an_undecided_create_asks_no_get_review_where_get_would_be_denied`,
  `a_create_denied_beside_its_own_error_is_named_before_the_undecided_get`,
  `a_get_denied_beside_an_error_after_a_denied_create_names_both_verbs`,
  `the_probe_names_a_denied_create_before_an_undecided_get`.
- `client_tests.rs`, `mod d364_adv2`:
  `an_access_review_error_is_bounded_in_characters_and_allowed_must_be_a_boolean`.

Runs: `-p b10x-substrate-kubernetes --lib` 170 passed; `-p b10x-substrate-kubernetes-client --lib`
56 passed; fmt on both crates exit 0. Clippy was refused verbatim:
`precheck: CHECK-FAILED 2 other cargo: avail=81G load1=27.37 psi60=0.66 others=2 telos=1 target=2G`.

Findings:

1. `workspace.rs:647` and `:612`, pre-existing, outside the Acceptance: when `create` is denied and
   the `get` review then fails, answers no `allowed`, or outlasts the 5 s bound, the refusal names
   the verb unknown and drops the `create` denial. The base `8b9818dc` does the same, and
   `d354_adv::a_second_review_that_fails_keeps_the_refusal_and_names_the_verb_unknown` pins it. No
   lane reaches it; no case written.
2. Design 34 line 895, introduced: the amendment says the combined line is not S6 "because `get` is
   open", where `get` is undecided and "open" reads as allowed. Line 316 is not wrapped like its
   neighbours.

Attacked and not broken: the lane's S6 matcher (`xtask/src/namespace_lane_run.rs:471`) still
compares the trimmed reason to `REVIEW_DENIES_CREATE`, and the combined line can never equal it; the
`denied.is_empty()` check is caught by both rows of the unit's case (reasoned, not run); token
clearing and the 300-character bound hold on the combined path; the bound counts characters.

```findings
[{"file": "crates/substrate-kubernetes/src/namespace/workspace.rs", "line": 647, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "pre-existing", "message": "scope out; case none: a create denial already collected is still dropped when the get review fails, answers no allowed or outlasts the 5 s bound, unlike the corrected evaluation-error branch, pinned by d354_adv::a_second_review_that_fails_keeps_the_refusal_and_names_the_verb_unknown and reached by no lane found"}, {"file": "docs/design/34-kubernetes-namespace-driver.md", "line": 895, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: the amendment says the combined line is not S6 because get is open, where get is undecided, and open reads as allowed"}]
```
