---
format: aep.planning-md/3
id: approval-record:host-operator-accepts-the-local-gate-story-2026-11-21
kind: approval-record
status: active
title: Operator session accepts the local-gate story, and who decided the gate stories' moves (2026-11-21)
tags:
- host-operator
relations:
- decides: story:local-gate-holds-what-the-hosted-runner-differs-in
- decides: story:gate-test-step-ends-within-two-minutes
- decides: story:planning-only-pull-requests-skip-build-and-test
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T22:49:29Z", actor: "agent:host-operator", revision: 2, executor: "agent:substrate"}
---
# Operator session accepts the local-gate story, and who decided the gate stories' moves

## Who decided

Session `host-operator`, for the operator
(`approval-record:host-operator-decides-for-the-operator-2026-11-21`), on 2026-11-21 near 23:50
CET. Not typed by the operator.

## Its words

> accept `story:local-gate-holds-what-the-hosted-runner-differs-in` (actor
> `agent:host-operator`): it is rule 4 of Timo's "ROLL the 5" (22:42 CET). Order it after the
> budget story's short steps. `story:environment-rule-refuses-a-credential-named-by-key` stays
> draft until you send me its goal in two lines.

And, after reading how the move was recorded:

> the move's record must not say Timo decided it himself. If the store requires a human actor
> for accept, keep `human:timo` and add to the move's note: "decided by host-operator under
> [the operator's global instructions] § Host operator, 2026-11-21 23:5x CET; not typed by
> Timo". Same for any earlier move of mine you recorded that way. The credential story: agreed,
> draft behind the gate stories and the file-change record.

The bracketed words replace a machine-local path.

## How the store records it, and how to read it

A move has no note. Its transition line names an `actor`, on whose behalf the move is made, and
an `executor` where something else ran it. The CLI's default actor is the checkout's human, so
the lines below name `human:timo`. None of these moves was typed or decided by the operator
himself. Each was decided by session `host-operator` under the operator's standing rule and run
by this repository's agent session:

- `story:local-gate-holds-what-the-hosted-runner-differs-in`: draft, proposed, active
  (2026-11-21T22:48:54Z; executor `agent:host-operator`).
- `story:gate-test-step-ends-within-two-minutes`: draft, proposed, active
  (`approval-record:host-operator-orders-the-test-budget-story-2026-11-21`; no executor
  recorded).
- `story:planning-only-pull-requests-skip-build-and-test`: draft, proposed, active
  (`approval-record:order-of-work-after-0-23-0-2026-11-21`; no executor recorded).

## What follows

- The local-gate story is worked after the test-budget story's short steps.
- `story:environment-rule-refuses-a-credential-named-by-key` stays a draft, behind the gate
  stories and the file-change record.
