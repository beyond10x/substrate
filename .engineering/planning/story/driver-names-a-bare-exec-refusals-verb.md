---
format: aep.planning-md/3
id: story:driver-names-a-bare-exec-refusals-verb
kind: story
status: active
title: The driver names a bare exec refusal's verb
relations:
- decomposes: epic:confinement-contract-family
- informed_by: approval-record:d349-substrate-wave-2026-11-01
scope:
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/api.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/client_tests.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/fake.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/namespace/verb_review_tests.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/workspace.rs
- confidence: inferred
  path: docs/design/34-kubernetes-namespace-driver.md
- confidence: inferred
  path: xtask/src/namespace_lane_run.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T10:16:18Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-01T15:13:33Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"health_observation":1}}}
---
# Story: the driver names a bare exec refusal's verb

## Observation

On kind v1.37 the API refuses an exec upgrade the Role lacks `pods/exec` `create` for with a
`Status` whose message carries no reason (the 0.18.1 namespace lane at `bb2b06fe`, daemon log kept
by the lane):

```
2026-11-01T10:08:42.729145Z  WARN substrate_kubernetes::namespace::probe: the namespace driver's probe failed; its facts are absent code="kubernetes.executor-handshake-failed" the cluster API: exec answered 403: pods "sbx-pr-gbyb7pajquihb757-keeper" is forbidden: 
2026-11-01T10:09:43.181817Z  WARN substrate_kubernetes::namespace::probe: the namespace driver's probe failed; its facts are absent code="kubernetes.executor-handshake-failed" the cluster API: exec answered 403: pods "sbx-pr-fma7x5qkyobdhq4t-keeper" is forbidden: 
```

An operator reading that warning cannot tell which permission is missing. D349 accepts the bare
line in the lane for that one case; this story makes the driver name the verb.

## Acceptance

- When an exec is refused 403 with no reason after "is forbidden:", the driver asks a
  `SelfSubjectAccessReview` for `pods/exec` `create` and `get` in its namespace and names the verb
  the review denies in the refusal, or says the review did not decide.
- The review's own refusal or failure keeps the original refusal and names that the verb is
  unknown; the handshake still ends at once.
- Design 30 states the extra API call; the chart's Role and the namespace lane's Role need no new
  rule if the cluster's default `system:basic-user` binding admits the review, and the story says
  which it found.

## Scope

Derived 2026-11-01 by `story-scoper` at `bf0e0443` (wave D354). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-kubernetes`, the namespace driver's refused-handshake path — cited
- **Files:** `crates/substrate-kubernetes/src/namespace/workspace.rs` (`await_handshake` 513, `handshake` 537, `Handshaken::Refused` 710, `refused_for_good` 719, Refused arm 550-551) — cited
- **Also likely:** `crates/substrate-kubernetes-client/src/api.rs`: a new `SelfSubjectAccessReview` method beside `rules_review` (689), built on the private `post` (443) — inferred
- **Also likely:** `crates/substrate-kubernetes-client/src/fake.rs`: an SSAR route beside `RULES_REVIEW` (74, served at 570); `refuse_exec` (415) fixes the reason to `Forbidden` (1121), so a bare refusal needs a new entry point — inferred
- **Also likely:** `crates/substrate-kubernetes-client/src/client_tests.rs`: the client method's test, beside the rules-review test (209) — inferred
- **Also likely:** `xtask/src/namespace_lane_run.rs`: `bare_exec_refusal` (about 441-460) requires an empty reason after `is forbidden:`; a driver message carrying the verb changes that rule and its D349 test (about 2749-2820) — inferred
- **Tests (red cases):** beside `a_refused_executor_handshake_names_the_apis_answer_at_once` in `crates/substrate-kubernetes/src/namespace/probe_tests.rs` (1767) — inferred; wave D354 puts them in a new module to stay off that file
- **Documents:** `docs/design/34-kubernetes-namespace-driver.md` § 6.1 step 7 (225), § 7.1 (297), § 10.1 (454), a bullet under `Amendments of 2026-11-01` (865) — inferred. Design 30 names no driver API call (cited); the Acceptance's "Design 30" means design 34.
- **Not touched:** `crates/substrate-kubernetes/src/rules.rs` — `DEFAULT_RESOURCE_RULES` already admits `selfsubjectaccessreviews` `create` from `system:basic-user` (36-38), cited; `deploy/helm/substrate/` has no Role (cited); `spec/` (cited).
- **Confidence:** medium (high for the crate surface; medium for the xtask touch).
- **Would collide with:** any unit editing `namespace/workspace.rs`, the client's `api.rs` or `fake.rs`, the lane's S6 rule in `xtask/src/namespace_lane_run.rs`, or design 34's amendments section.
