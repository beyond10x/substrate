---
format: aep.planning-md/3
id: story:access-review-evaluation-error-is-not-a-denial
kind: story
status: proposed
title: A SelfSubjectAccessReview with an evaluation error is not read as a denial
relations:
- informed_by: review-result:wave-2026-11-01-d354-u2-adversary-pass-1
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
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
