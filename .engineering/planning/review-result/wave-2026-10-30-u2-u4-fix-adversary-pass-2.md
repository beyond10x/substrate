---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u2-u4-fix-adversary-pass-2
kind: review-result
status: active
title: Adversary pass 2, the fix unit for u2 and u4
relations:
- reviews: story:kubernetes-collisions-metric
- reviews: story:unconfirmed-pod-deletion-ends-unknown
revision: 1
---
# Adversary pass 2, the fix unit for u2 and u4

Agent type `aep:adversary`. Tree on integration head `1a36bf66` (fix commits `7ea75a55`, collisions,
and `a00bb417`, unconfirmed deletion). Seven cases committed in
`crates/substrate-kubernetes/src/driver_tests.rs` and `exec_tests.rs` (`7d071077`, merged as
`93d8cceb`), all green. `cargo test -p b10x-substrate-kubernetes --lib`: 109 passed. Verdict: green.

```findings
- file: crates/substrate-kubernetes/src/namespace.rs
  line: 486
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: When the 409 arm's read of the replacement fails, the replacement is counted unread and again under its uid by the next removal, two for one object; the doc on collision() accepts this overcount.
- file: crates/substrate-kubernetes/src/ownership.rs
  line: 43
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: A foreign object without a uid is keyed on kind and name alone, so a second uid-less object under that name is never counted; no real API server or the fake produces one.
- file: crates/substrate-kubernetes/src/namespace/workspace.rs
  line: 367
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: exec_rows_owned_by stops collecting at the first ownerless Pod, so a second foreign Pod using the workspace is counted only after the first is gone.
```

Held: `Collisions::meet` is synchronous and no lock is held across an await; `deliver_tracked`
with its own read failing returns the error and records nothing; a lost answer whose read shows
another uid or no `deletionTimestamp` records nothing; the watch round's re-read racing a DELETED
event ends Unknown once; a term before the bound keeps the earliest acceptance.

Coordinator decision: no change. The overcount is the documented safe direction, the uid-less key
is unreachable, and the late count of a second ownerless Pod is pre-existing and never a miss.
