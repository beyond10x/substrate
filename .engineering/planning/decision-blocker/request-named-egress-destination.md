---
format: aep.planning-md/3
id: decision-blocker:request-named-egress-destination
kind: decision-blocker
status: cleared
title: Whether a launch may name its egress destination, against ADR 0013
relations:
- blocks: story:request-named-egress
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-28T18:06:40Z", actor: "human:timo", revision: 3}
---
# A consumer asks for egress to a destination the request names

## The ask

A consumer (loom, `story:child-launches-through-confinement-adapters`) launches child processes
through Substrate's confinement adapters. It asks for egress to be given as a full destination —
host, port and address — on the launch, where today the launch names operator-declared apertures.

## Why this is a decision and not a story

[ADR 0013](../../../adr/0013-egress-apertures-are-declared-by-the-operator.md) decides the opposite:
"A request may reference an aperture **by name** and may never carry a destination, at any depth, in
any field. Configuration owns reach; a request selects among what configuration already
permitted." A raw destination where a name belongs is a named refusal today. Accepting the ask moves
reach from deployment authority to request authority, which is invariant 3's floor and O1's
*may reach*.

## Options

| | what it does | cost |
|---|---|---|
| A | Keep ADR 0013. The consumer's operator declares one aperture per destination it needs and the consumer maps host:port to the aperture name. | No Substrate change. The consumer needs a mapping and a declaration step per destination. |
| B | A new ADR adds an operator-declared **destination set** (host patterns, port ranges) that a request may name a member of; the daemon checks the requested tuple against the set and records it applied. | Successor bundle, new fact, new refusals; DNS pinning per request; the delegated lane must prove an out-of-set tuple unreachable. |
| C | Supersede ADR 0013: a request carries any destination. | Removes the operator bound on reach. Conflicts with O1. |

Recommendation: A, and B only if a concrete consumer cannot enumerate its destinations in advance.

## What clears it

An accepted decision (an ADR for B or C, or a recorded "A stands" answer to the consumer). Until
then no implementation story for request-named egress is accepted.


## Decided, 2026-10-28: A

ADR 0013 stands. The consumer's operator declares one aperture per destination it needs, and the
consumer maps each host:port to the declared aperture name. Taken by the coordinating session as
its D160, because option A changes no ADR; reversing ADR 0013 (option C) or adding a destination
set (option B) would need an accepted ADR first. `story:request-named-egress` is archived with
this answer.
