---
format: aep.planning-md/3
id: story:commands-that-read-before-a-mutation-replay-their-stored-outcome
kind: story
status: draft
title: A retried CLI command replays its stored outcome when its resource is gone
tags:
- defect
scope:
- confidence: inferred
  path: crates/b10x-substrate-cli/src/lib.rs
- confidence: cited
  path: crates/b10x-substrate-cli/tests/scenarios.rs
- confidence: inferred
  path: crates/b10x-substrate-sdk/src/lib.rs
- confidence: cited
  path: scripts/delegated-lane.sh
revision: 4
---
# Story: a retried CLI command replays its stored outcome when its resource is gone

## Outcome

`workspace renew`, `exec run` and `session signal` run a second time with the same
`--operation-id` print the stored result, as `workspace destroy` does since release 0.25.0.

## What is known

Release 0.25.0's changelog states it: those three commands read their resource before the
mutation, and a retry after the resource is gone answers the read's `resource.not-found`.

## What changes

Each command submits its mutation by id and operation id without the read, through an SDK call
that takes the id, as `Client::destroy_workspace` does. The MCP adapter's tools with the same
shape are checked and listed.

## Acceptance

One CLI case per command against the shipped daemon: the command, the resource removed, the
same command again; both exit 0 and print the same result.


## Done, 2026-11-24

The three commands send their mutation by id through `Client::renew_workspace_lease`,
`Client::signal_pipe_session` and `Client::command_in_workspace` and read nothing first. The
SDK's public surface only gains items. Cases in `crates/b10x-substrate-cli/tests/scenarios.rs`,
against the shipped daemon:

- one per command, as the acceptance asks: the command, the resource removed, the same command
  again;
- a workspace that never existed: the refused start is recorded under its operation id and
  replayed (the portable assertion for `exec run`, which needs no confined exec);
- one operation id names one renewal, one signal and one run: other arguments under it are
  `operation.request-conflict`, and the first request still replays;
- a replayed `exec run` keeps its exit status, both truncation flags and a timeout;
- one limit, held by a `limit_` case: after the daemon published another capability snapshot (a
  daemon start does), a retried `exec run` is refused `operation.request-conflict`, nothing is
  dispatched again, and the stored outcome is read with `operation show`. The renewal replays
  across the same restart. `story:a-retried-exec-start-replays-across-a-daemon-restart` asks
  whether the start's request identity should hold the snapshot.

The MCP adapter's tools with the same shape are listed in
`story:mcp-tools-that-read-before-a-mutation-replay-their-stored-outcome` and are unchanged.
