---
format: aep.planning-md/3
id: approval-record:design-34-accepted
kind: approval-record
status: active
title: Operator accepts design 34; Q1 B, Q2 A with a lane observation, Q3 A
relations:
- decides: story:kubernetes-namespace-workspace-and-exec-slice
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-29T01:46:15Z", actor: "human:timo", revision: 2}
---
# Decision: design 34 accepted, with a lane observation added to Q2

Taken 2026-10-29 by the operator, relayed by the coordinating session. The operator's answer,
verbatim:

> "accept all if it s the right and not the cheapest low effort choice"

The coordinating session read design 34 § 14 against that condition:

| Question | Answer | Why |
|---|---|---|
| Q1, additive NetworkPolicies | B | one read-only verb, and every policy change moves `config_generation`; amends ADR 0047 decision 3's table |
| Q2, the process bound | A, plus observation O11 | A alone states a bound without observing it; O11 spawns processes in an exec Pod until `fork` fails and records the count against `pod_pids_limit`, and the release asserts it |
| Q3, missing refusals | A | B calls a valid request malformed |

Q2 C stays rejected: a per-uid count is the wrong bound.

## Condition

If O11 shows the bound does not hold, A does not ship: the lane output goes to the coordinating
session first.

## What follows

Design 34 is accepted for implementation and lands with the next wave pull request (D169). Its
slices start in the wave after `wave/adapter-io`.
