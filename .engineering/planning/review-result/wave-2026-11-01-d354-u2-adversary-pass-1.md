---
format: aep.planning-md/3
id: review-result:wave-2026-11-01-d354-u2-adversary-pass-1
kind: review-result
status: active
title: D354 U2 adversary pass 1
relations:
- reviews: story:driver-names-a-bare-exec-refusals-verb
revision: 1
---
# Adversary pass 1 — wave D354 U2 (story:driver-names-a-bare-exec-refusals-verb)

Diff `91c8163a...ec461246`; adversary commits `c1b66d7b`, `79b7fb0d` on `unit/d354-u2`, merged into the integration branch at `e2bc89d5`. Agent `aep:adversary`. Header and findings verbatim; prose trimmed to the measured facts.

```
verdict: green
cases: executed 224→230, red 0
origin: introduced 4, pre-existing 0, undecided 0 (adversary)
```

Commands, each after the wave pre-check: kubernetes --lib verb_review 11/0; xtask namespace_lane_run 27/0; client --lib 53/0; kubernetes --lib 150/0; fmt clean; clippy -D warnings clean.

Cases:

- `two_reviews_that_together_outlast_five_seconds_leave_the_verb_unknown` (`verb_review_tests.rs:289`): the 5 s bound covers both reviews together.
- `verb_review_tests.rs:318`: a `get` review answering 500 after the `create` review answered keeps the API's refusal and names the verb unknown, after exactly 2 reviews.
- `verb_review_tests.rs:350`: the review names the refused keeper; `resourceNames` decides between "does not decide" and "denies create".
- `verb_review_tests.rs:381`: empty, space, tab and newline after `is forbidden:` count as bare; the API's text is kept.
- `verb_review_tests.rs:400`: a reason as short as `x` asks no review.
- `the_create_cases_review_line_is_the_drivers_own_words` (`xtask/src/namespace_lane_run.rs`): the lane's line is read from the driver's constant.

```findings
[{"file": "xtask/src/namespace_lane_run.rs", "line": 443, "category": "contract-drift", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case the_create_cases_review_line_is_the_drivers_own_words (green, 79b7fb0d): the lane's REVIEW_DENIES_CREATE copied the driver's ACCESS_REVIEW_DENIES with nothing tying them, so a reworded driver line would have broken the S6 create case only on the live lane; the case now ties them."},
{"file": "crates/substrate-kubernetes/src/namespace/workspace.rs", "line": 627, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: allowed:false is read as a denial even when the review carries status.evaluationError or no authorizer opinion, which the Acceptance's 'did not decide' could cover; nothing found reaches it on kind's RBAC-only setup."},
{"file": "crates/substrate-kubernetes/src/namespace/workspace.rs", "line": 788, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: a 403 with an empty Status.message, or with only punctuation after 'is forbidden:', asks no review; Kubernetes' forbidden format emits neither, nothing found reaches it."},
{"file": "docs/design/34-kubernetes-namespace-driver.md", "line": 42, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope out; design 34 section 2's crate table and section 10.1's fake list (line 471) name SelfSubjectRulesReview but not the new SelfSubjectAccessReview client method and fake route."}]
```

Resolution: finding 1 is fixed by its own case (79b7fb0d). Finding 4 is fixed in the integration branch (§ 10.1 in 1ecd4f74, § 2 in 371cd1fc). Findings 2 and 3 are decided at wave close.
