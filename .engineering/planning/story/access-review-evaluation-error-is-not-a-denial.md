---
format: aep.planning-md/3
id: story:access-review-evaluation-error-is-not-a-denial
kind: story
status: active
title: A SelfSubjectAccessReview with an evaluation error is not read as a denial
relations:
- informed_by: review-result:wave-2026-11-01-d354-u2-adversary-pass-1
scope:
- confidence: cited
  path: crates/substrate-kubernetes-client/src/api.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/client_tests.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/namespace/verb_review_tests.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/workspace.rs
- confidence: inferred
  path: docs/design/34-kubernetes-namespace-driver.md
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T23:08:45Z", actor: "human:timo", revision: 6}
---
# Story: a SelfSubjectAccessReview with an evaluation error is not read as a denial

## Outcome

When the namespace driver asks a `SelfSubjectAccessReview` about a bare exec refusal and the review
answers `allowed: false` with `status.evaluationError` set, the driver says the review did not
decide, rather than naming the verb denied.

## Observation

Wave D354 adversary pass on U2 (`review-result:wave-2026-11-01-d354-u2-adversary-pass-1`, finding 2):
`crates/substrate-kubernetes/src/namespace/workspace.rs:627` and
`crates/substrate-kubernetes-client/src/api.rs:749` read `allowed: false` alone as a denial. With a
webhook authorizer that had no opinion, or an evaluation error, that is not a decision. Kind's
RBAC-only setup never produces it; inferred from the API's semantics, not observed.

## Acceptance

- A fake review answering `allowed: false` with `evaluationError` makes the refusal say the review
  did not decide, and the API's answer is kept.
- A review answering `allowed: false` with no error still names the denied verb.

## Scope

Derived 2026-11-02 by `story-scoper` (wave D364). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-kubernetes-client` (the answer is read) and `crates/substrate-kubernetes` (the refusal text is decided), cited: story Observation, `api.rs:749`, `workspace.rs:627`.
- **Client:** `api.rs:722` `access_review` returns `Result<bool, ApiError>` and reads only `status.allowed` at `:749`, so `evaluationError` is dropped before the driver sees it, cited.
- **Driver:** `workspace.rs:627` `Ok(false) => denied.push(verb)` inside `denied_exec_verbs` (`:622`); the verdict match at `:600-612`, cited.
- **Symbols:** `refused_handshake` (`workspace.rs:594`), `is_bare_refusal` (`:788`, the gate, not edited), cited.
- **Callers of `access_review`:** `workspace.rs:625` and `client_tests.rs:245,279,299`, the only four in the tree; a return-type change touches all four, cited.
- **Fake:** `fake.rs:584-590` answers only `{"allowed": …}`; `Fault::Answer` (`fake.rs:539`, one-shot) already carries a custom body on this path, as `client_tests.rs:289-299` does, so no fake change is needed, cited.
- **Tests:** a new case in `crates/substrate-kubernetes/src/namespace/verb_review_tests.rs` beside `:125`; acceptance 2 is already covered by `verb_review_tests.rs:101`, inferred placement. A client case beside `crates/substrate-kubernetes-client/src/client_tests.rs:264`, inferred.
- **Documents:** design 34 § 7.1 (`docs/design/34-kubernetes-namespace-driver.md:300-318`) states the verdicts and never mentions `evaluationError`, so it needs one line and an amendments bullet; cited for the silence, inferred for the need.
- **Not touched:** `xtask/src/namespace_lane_run.rs` `bare_exec_refusal` (`:453`) matches only `REVIEW_DENIES_CREATE`; the new undecided text is a different constant, inferred.
- **Confidence:** medium. Every site is read from the tree; the fix's shape (a new return type or a new method) is a design choice.
- **Would collide with:** any unit editing `workspace.rs` (`:589-636`), `api.rs` `access_review`, or design 34 § 7.1 and its amendments.
