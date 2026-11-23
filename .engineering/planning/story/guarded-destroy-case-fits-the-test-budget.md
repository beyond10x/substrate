---
format: aep.planning-md/3
id: story:guarded-destroy-case-fits-the-test-budget
kind: story
status: draft
title: The guarded crate's destroy case runs within the test budget
tags:
- test-budget
scope:
- confidence: cited
  path: crates/substrate-guarded/src/fs.rs
revision: 2
---
# Story: the guarded crate's destroy case runs within the test budget

## Outcome

`fs::tests::destroy_batches_progress_beyond_former_depth_and_item_caps` ends within 60 s on a
loaded workstation and the guarded crate's test step within 120 s.

## What was measured

2026-11-23, release profile, host load average about 100: the case alone took 70.28 s. The
whole package took 67.06 s in one run and 44.56 s in another (102 cases). The collection's
budget is 60 s for one test (`AGENTS.md` of the collection, § Tests stay fast).

## What changes

The case builds a tree deeper and wider than the former caps to prove progress. It proves the
same rule with a tree just past each cap, built once, and states the counts it needs.

## Acceptance

The case under 20 s alone on a loaded host; the same assertions on depth and item count.

