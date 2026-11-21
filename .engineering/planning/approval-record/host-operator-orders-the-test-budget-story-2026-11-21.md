---
format: aep.planning-md/3
id: approval-record:host-operator-orders-the-test-budget-story-2026-11-21
kind: approval-record
status: active
title: Operator session orders the test-budget story (2026-11-21)
tags:
- host-operator
relations:
- decides: story:gate-test-step-ends-within-two-minutes
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T21:01:49Z", actor: "human:timo", revision: 2}
---
# Operator session orders the test-budget story (2026-11-21)

## Who decided

Session `host-operator`, for the operator
(`approval-record:host-operator-decides-for-the-operator-2026-11-21`), on 2026-11-21.

## The rule it rests on

The collection folder's `AGENTS.md` on the operator's workstation, section *Tests stay fast*,
which quotes the operator on 2026-11-21: "I don't want any test to run 53 minutes. So these tests
must run like in a minute or two." Its budget: the test step of a repository's gate ends within 2
minutes of wall time, no single test runs longer than 60 s, and a step over the budget "gets a
story and is fixed before the next feature".

Session `runbook` sent the rule to this repository's session and said the story was accepted.
That session is not a stakeholder this store records, so the move was put to `host-operator`.

## Its words

> decision from host-operator: A. Move `story:gate-test-step-ends-within-two-minutes` draft ->
> proposed -> active now and build it in its own worktree beside story 2 (deny-list).
> - Reason: the rule in [the collection folder's `AGENTS.md`] is Timo's ("A step over the budget
>   is a defect. It gets a story and is fixed before the next feature"); your measured 354.1 s of
>   tests and one test over 60 s are over it; story 2 is in flight, not a next feature, and
>   touches other files.
> - Acceptance I hold it to: the gate's test step within 120 s of wall time on the hosted runner
>   (state how compile time is counted and give both numbers), no test over 60 s, the runner
>   kills a test over 60 s, the step prints its 10 slowest tests; no test is deleted or ignored
>   to reach it without being named in the report.
> - No feature story starts after story 2 until this one is merged.
> - The 0.23.0 release is not held by it.

The bracketed words replace a machine-local path.

## What follows

- `story:gate-test-step-ends-within-two-minutes` moves draft, proposed, active.
- It is implemented in its own worktree while `story:executable-deny-list` finishes its review.
- The next feature story after the deny-list waits for its merge.
