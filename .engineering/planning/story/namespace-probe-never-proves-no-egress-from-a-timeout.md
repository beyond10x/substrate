---
format: aep.planning-md/3
id: story:namespace-probe-never-proves-no-egress-from-a-timeout
kind: story
status: implemented
title: The namespace probe never proves no-egress from a timed-out attempt
owner: substrate
tags:
- confinement
- kubernetes
relations:
- informed_by: approval-record:d333-timeout-never-proves-no-egress
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T18:33:18Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-31T18:33:18Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-31T18:51:32Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
# Story: the namespace probe never proves no-egress from a timed-out attempt

## Outcome

The namespace driver publishes `confinement.no-egress` only when each of the probe's three egress
attempts was denied locally (`EPERM`, `EACCES`, `ENETUNREACH` or `EHOSTUNREACH`). An attempt that
timed out, was reached, was refused or failed otherwise proves nothing, and the fact stays absent.

## Design

D332 traced the 0.18.0 namespace lane at `3b3998c8` (`approval-record:d332-no-egress-fix-wave`):
one probe saw its three attempts time out and published the fact while the lane's own Pod under
the same policy reached all three targets. D333 (`approval-record:d333-timeout-never-proves-no-egress`)
chose option C: this story now, `story:namespace-probe-repeats-and-sticks-on-egress` later.
Design 34 § 6.2 is amended to match.

## Acceptance

- A probe test reproducing trace 1 (reached, reached, refused) withholds the fact.
- A probe test reproducing trace 2 (three timeouts) withholds the fact; it fails before the fix.
- A probe test with three local denials publishes the fact.
- `EgressOutcome::blocked` is true only for `Denied` and `Unreachable`.
