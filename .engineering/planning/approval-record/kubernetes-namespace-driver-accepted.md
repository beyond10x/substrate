---
format: aep.planning-md/3
id: approval-record:kubernetes-namespace-driver-accepted
kind: approval-record
status: draft
title: Operator accepts ADR 0047 with every recommended answer
relations:
- decides: story:kubernetes-namespace-driver-entry-gate
revision: 1
---
# Decision: ADR 0047 accepted with every recommended answer

Taken 2026-10-29 by the operator, relayed by the coordinating session. The operator's answer,
verbatim: "A".

Option A was: accept ADR 0047 with the recommended answer to each of N1 to N12, as written at
commit `ce3fb77c` on branch `adr/kubernetes-namespace-driver`.

## What follows

- ADR 0047 moves to `accepted` and lands with the next wave pull request (D169).
- The namespace driver's implementation design and code go in the wave after `wave/adapter-io`;
  the accepted design comes before code (invariant 8).
- The four behaviours ADR 0047 § *Consequences* lists as unobserved are checked on a disposable
  kind lane before the implementation relies on them.
