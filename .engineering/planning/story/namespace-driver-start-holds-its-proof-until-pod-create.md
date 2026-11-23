---
format: aep.planning-md/3
id: story:namespace-driver-start-holds-its-proof-until-pod-create
kind: story
status: draft
title: A start on the namespace driver is refused when its confinement proof no longer holds at Pod creation
summary: The driver compares digest, required facts, node and validity again before it creates the Pod; admission reads one snapshot
tags:
- defect
- security
revision: 2
---
# Story: a start on the namespace driver is refused when its confinement proof no longer holds at Pod creation

## Outcome

An exec on the namespace driver runs only under the confinement proof it was admitted with. When
the published proof changed between admission and the creation of the Pod, the start is refused
by name and no Pod is created. Daemon admission takes facts and digest from one snapshot.

## What changes

1. `crates/substrate-kubernetes/src/exec.rs`: immediately before the Pod is created, after every
   wait the start can sit in, the driver compares the admission snapshot's digest, the facts the
   start requires, the node and the validity with the latest publication.
2. `crates/substrate-daemon/src/app/operations.rs`: one call to `machine()` per admission.

## Design

No wire schema or capability fact changes; the refusal is one the driver already raises for a
stale snapshot. Design 34 gets the rule where it describes the start.

## Acceptance

- A case on the crate's fake cluster: a pending start is refused when a newer publication on the
  same node lacks a fact the start requires; red before the change. Estimated 0.5 to 3 s.
- A case that admission passes one snapshot's facts and digest to the driver.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 3 (high priority). The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.

## Delivery

## Delivery

Branch `story/namespace-proof`, pull request 263. Reviewed twice
(`review-result:namespace-proof-security-1`, `review-result:namespace-proof-security-2`). The
first review replaced the rule of the first commit: a start is held to facts, node, Lease
epoch, binding and policy set, and the admission's digest is looked up in the driver's memory
of 2,048 stated snapshots, never compared. The second review found the comparison in place
after the last wait; its three findings are closed: a proof ends when either clock says so, a
digest remembered with two statements is stale, and the design states what 2,048 spans.

Open after this story, on `main` too: two older test cases race a sleep against a delayed read
and can fail when the test thread stalls 300 to 500 ms (`exec_tests.rs`, `driver_tests.rs`).
