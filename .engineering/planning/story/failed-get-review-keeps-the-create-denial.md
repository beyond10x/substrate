---
format: aep.planning-md/3
id: story:failed-get-review-keeps-the-create-denial
kind: story
status: active
title: A failed get review keeps a create denial named
relations:
- informed_by: review-result:wave-2026-11-02-d364-u2-adversary-pass-2
- informed_by: approval-record:wave-2026-11-04-substrate
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-02T01:25:02Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-04T07:33:32Z", actor: "human:timo", revision: 3}
---
# Story: a failed get review keeps a create denial named

## Outcome

When the Kubernetes namespace driver's `SelfSubjectAccessReview` for `pods/exec` `create` denies
and the following `get` review is refused, fails, answers no `allowed` or outlasts the 5 s bound,
the bare exec refusal still names the `create` denial before saying that the refused verb is
unknown, as the evaluation-error branch does since wave D364.

## Observation

Wave D364 adversary pass 2 on U2 (`review-result:wave-2026-11-02-d364-u2-adversary-pass-2`, origin
pre-existing, scope out): `crates/substrate-kubernetes/src/namespace/workspace.rs:612` and `:647`
drop an already collected `create` denial on the failure branch and name the verb unknown. The base
before D364 does the same, and
`d354_adv::a_second_review_that_fails_keeps_the_refusal_and_names_the_verb_unknown` pins it. No lane
was found that reaches it.

## Design

Carry the denied verbs into the failure branch the way `denied_exec_verbs` carries them into the
undecided branch; amend the pinning case and design 34 § 7.1 under this story.

## Acceptance

- A case denies `create`, then fails the `get` review (refused, no `allowed`, past 5 s); the
  refusal names the `create` denial before it says the refused verb is unknown and keeps the API's
  answer.
