---
format: aep.planning-md/3
id: story:host-crate-library-cases-fit-the-test-budget
kind: story
status: draft
title: The host crate's library cases fit the test budget on a loaded workstation
tags:
- test-budget
scope:
- confidence: inferred
  path: crates/substrate-host/src
revision: 2
---
# Story: the host crate's library cases fit the test budget on a loaded workstation

## Outcome

A run of `cargo test -p b10x-substrate-host --lib` ends within the collection's 120 s step
budget on the shared workstation, or the cases that cannot are named and moved to a lane.

## What was seen

2026-11-23, workstation, debug profile, libtest's default threads, load average 40 to 80:
634 cases passed in 179.34 s; 635 passed in 197.15 s and in 237.54 s. No single case was
reported over 60 s. The gate's test step runs the same cases under cargo-nextest in the
release profile with 12 threads and was inside 120 s on the hosted runner that day.

## Where the requirement comes from

The collection's rule "Tests stay fast": a step over the budget is a defect, and a
whole-workspace run known to exceed the budget is not started on the shared machine.
Stakeholder: the operator.

## What changes

1. Time the same cases under cargo-nextest in the release profile on the workstation at a
   stated load, and print the ten slowest.
2. Name the cases that hold the time (candidates, not measured: cases that start a process
   and wait for a timeout, and cases that probe the host).
3. An expensive product made once per case becomes a setup step or a cache under `target/`;
   a case that waits for wall time gets a shorter bound.

## Acceptance

The library cases run within 120 s under cargo-nextest on the workstation at a load average
of 40 or more, recorded with the load, and no case runs over 60 s.
