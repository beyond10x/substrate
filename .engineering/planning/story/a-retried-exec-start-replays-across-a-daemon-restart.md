---
format: aep.planning-md/3
id: story:a-retried-exec-start-replays-across-a-daemon-restart
kind: story
status: draft
title: A retried exec start replays its stored outcome across a daemon restart
tags:
- design-question
relations:
- informed_by: review-result:cli-replay-adversary
scope:
- confidence: inferred
  path: crates/b10x-substrate-sdk/src/lib.rs
- confidence: inferred
  path: crates/substrate-daemon/src/app/operations.rs
revision: 2
---
# Story: a retried exec start replays its stored outcome across a daemon restart

## Outcome

It is decided whether an exec start retried with its operation id after the daemon published
another capability snapshot replays the stored outcome, and the daemon, the SDK and the CLI do
what was decided.

## What holds today

The start request names the capability snapshot the client read at connect
(`sandbox.capability_snapshot`). The snapshot's identity holds the instant of the probe, so each
daemon start publishes another one. A retry after a restart is therefore another request under
the same operation id, and the daemon answers `operation.request-conflict`. Nothing is
dispatched a second time, and the stored outcome is read with `operation show <id>` and
`exec show <id>`. `workspace renew`, `workspace destroy` and `session signal` name no snapshot
and replay across a restart.
`limit_exec_run_retried_under_another_capability_snapshot_is_a_request_conflict` in
`crates/b10x-substrate-cli/tests/scenarios.rs` holds it. The MCP adapter's `exec_start` names
the snapshot too.

## Where the requirement comes from

The adversary pass on the CLI commands that send their mutation by id
(`review-result:cli-replay-adversary`). The concept page on operations gives a lost response as
the reason to retry with the operation id, and a daemon restart is one way to lose a response.
Stakeholder: the operator.

## The question

Invariant 8: the answer is a design decision before code. Options seen, none chosen:

- A. The identity of a start request leaves the snapshot out. The daemon still refuses a first
  start that names a snapshot it does not publish; a retry whose other members are equal
  replays. Changes what the wire calls one request: a successor bundle text, no new route.
- B. The SDK, on `operation.request-conflict` for a start it sent, reads the operation's record
  and returns its stored outcome when the record's request equals its own but for the snapshot.
  No wire change; every other client keeps the conflict.
- C. Nothing changes; the limit stays documented.

The safety envelope binds a capability snapshot to the backend's identity. Whichever option is
taken states what a caller may conclude from a replayed start about the facts the run was
started under.

## Acceptance

An approval record names the option. For A or B: the `limit_` case becomes the case that holds
the replay, in both lanes, and a first start that names a snapshot the daemon does not publish
is still refused, with a case.
