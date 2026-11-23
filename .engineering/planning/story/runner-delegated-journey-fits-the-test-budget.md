---
format: aep.planning-md/3
id: story:runner-delegated-journey-fits-the-test-budget
kind: story
status: draft
title: The clean-room runner's delegated lane fits the test budget
tags:
- test-budget
scope:
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
revision: 2
---
# Story: the clean-room runner's delegated lane fits the test budget

## Outcome

No test function of `crates/substrate-daemon/tests/runtime_vectors.rs` runs longer than 60 s.

## What was measured

2026-11-23: the delegated lane is one test function. In a delegated scope it took 74.54 s in
one run and 53.11 s in another, with 201 cases. The collection's budget is 60 s for one test.

## What changes

The lane's cases are grouped into several test functions that share one built daemon and each
start their own instance, or the lane's daemon is started once per binary by a setup step.
`DELEGATED_CASES` stays the count of cases and stays asserted.

## Acceptance

`bash scripts/delegated-lane.sh` reports the same case count; the slowest test function of the
runner is under 60 s on a loaded host, with its time in the step's list of slowest tests.

