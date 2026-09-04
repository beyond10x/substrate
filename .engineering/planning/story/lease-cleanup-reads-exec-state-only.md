---
format: aep.planning-md/1
id: story:lease-cleanup-reads-exec-state-only
kind: story
status: implemented
title: Workspace lease cleanup reads exec ids and states without output blobs
summary: execs_for_workspace loads stdout/stderr blobs the caller never reads; worst case 4 GiB per sweep (store execs.rs:545, service.rs:688).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/service.rs
- confidence: cited
  path: crates/substrate-store/src/execs.rs
- confidence: cited
  path: crates/substrate-store/src/lib.rs
- confidence: cited
  path: crates/substrate-store/src/tests.rs
revision: 8
---
# Story: Lease cleanup reads exec state only

## Context

Workspace lease cleanup calls `execs_for_workspace`, which loads every exec row of the workspace
through `load_exec`, including the `stdout` and `stderr` blobs
(`crates/substrate-store/src/execs.rs:545`). The caller reads only `resource.state` and
`resource.id` (`crates/substrate-daemon/src/app/service.rs:688-700`). The bounds allow 2048 execs
per subject (`crates/substrate-wire/src/lib.rs:28`) with 1 MiB per stream (`:22`), so one expiring
workspace can pull 4 GiB into daemon memory in one sweep. Code reading only.

## Acceptance

A workspace lease expiry over the maximum exec count completes with the daemon's resident memory
growth bounded by the exec metadata alone, proven by a store query that returns `(id, state)` and
a test asserting no output column is read on that path.

## Notes

`workspace_has_nonterminal_execs` in `store/workspaces.rs` already parses only `resource_json`;
the same shape serves here.

## Parallel work

This story shares `crates/substrate-daemon/src/app/service.rs` with story:metrics-streams-are-bounded and story:lease-cleanup-reads-exec-state-only; the two touch different functions (stream limits on `App` versus `cleanup_expired`) but land on one file, so they are worked in sequence, not at once.
