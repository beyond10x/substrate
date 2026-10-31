---
format: aep.planning-md/3
id: story:namespace-probe-repeats-and-sticks-on-egress
kind: story
status: proposed
title: The namespace probe repeats its egress attempts and withholds no-egress once any got through
owner: substrate
tags:
- confinement
- kubernetes
relations:
- informed_by: approval-record:d333-timeout-never-proves-no-egress
- depends_on: story:namespace-probe-never-proves-no-egress-from-a-timeout
- informed_by: approval-record:d334-only-a-local-denial-proves-blocked
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T18:33:19Z", actor: "human:timo", revision: 2}
---
# Story: the namespace probe repeats its egress attempts and withholds no-egress once any got through

## Outcome

On a network plugin that drops denied packets silently, the namespace driver can still state
`confinement.no-egress`, by repeating its probe in several fresh Pods over time, each attempting at
its first instant, and publishing the fact only when every attempt of every run timed out. Any
attempt reached or refused withholds the fact for the binding generation.

## Design

Option B of D333 (`approval-record:d333-timeout-never-proves-no-egress`). The evidence is the two
D332 traces at `3b3998c8` (`approval-record:d332-no-egress-fix-wave`):

- Trace 1: Kubernetes Service reached in 0 ms, nameserver reached in 21 ms, egress probe address
  refused (errno 111).
- Trace 2: all three attempts timed out (3001 ms, 3058 ms with errno 11, 3001 ms), while the lane's
  own Pod under the same default-deny policy reached all three targets.

Inferred, not verified: kind's network plugin enforces NetworkPolicy only after a Pod's first
moments, so an exec Pod may egress early. The design 34 amendment this needs, including how the
start window is bounded, is decided when this story is picked, not now.

## Acceptance

To be stated with the design 34 amendment.

## No route since D334

Since D334 (`approval-record:d334-only-a-local-denial-proves-blocked`), only `EPERM` or `EACCES`
counts as blocked: `ENETUNREACH` and `EHOSTUNREACH` can come from a remote router's ICMP host or
net unreachable or from an address of the wrong family. This story also decides whether reading
an attempt's errno together with its elapsed time can tell a local no-route (answered at once,
inside the Pod) from a remote one, and if so how `no_egress()` reads it.
