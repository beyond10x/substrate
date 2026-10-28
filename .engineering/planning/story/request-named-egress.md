---
format: aep.planning-md/3
id: story:request-named-egress
kind: story
status: draft
title: A confined child reaches a destination its launch names
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# A confined child reaches a destination its launch names

## Consumer ask

A consumer launching children through the confinement adapters (loom,
`story:child-launches-through-confinement-adapters`) asks to give egress as a full host, port and
address on the launch. Today a launch names operator-declared apertures.

## Blocked

`decision-blocker:request-named-egress-destination` blocks this story: ADR 0013 refuses a
destination in any request field. No design is written until that decision is taken.

## Acceptance

Written once the decision is recorded. If ADR 0013 stands, this story is rejected with that
answer recorded.
