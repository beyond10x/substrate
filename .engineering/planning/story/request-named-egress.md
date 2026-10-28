---
format: aep.planning-md/3
id: story:request-named-egress
kind: story
status: archived
title: A confined child reaches a destination its launch names
relations:
- decomposes: epic:confinement-contract-family
revision: 3
transitions:
- {from: "draft", to: "archived", at: "2026-10-28T18:06:40Z", actor: "human:timo", revision: 3}
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


## Decided, 2026-10-28: A

ADR 0013 stands. The consumer's operator declares one aperture per destination it needs, and the
consumer maps each host:port to the declared aperture name. Taken by the coordinating session as
its D160, because option A changes no ADR; reversing ADR 0013 (option C) or adding a destination
set (option B) would need an accepted ADR first. `story:request-named-egress` is archived with
this answer.
