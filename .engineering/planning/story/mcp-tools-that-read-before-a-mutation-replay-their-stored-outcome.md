---
format: aep.planning-md/3
id: story:mcp-tools-that-read-before-a-mutation-replay-their-stored-outcome
kind: story
status: draft
title: An MCP tool that mutates replays its stored outcome when its resource is gone
tags:
- defect
relations:
- informed_by: review-result:cli-replay-adversary
scope:
- confidence: cited
  path: crates/substrate-mcp/src/surface.rs
revision: 2
---
# Story: an MCP tool that mutates replays its stored outcome when its resource is gone

## Outcome

A tool of the MCP adapter that sends a mutation with an operation id sends it by id, without a
read of the resource first, so the same call made again after the resource is gone reaches the
daemon, which replays the stored outcome.

## What is known

Read, not run, 2026-11-24, in `crates/substrate-mcp/src/surface.rs`. Each tool takes the
caller's `operation_id`, passes the adapter's own registry check first (`require_workspace` or
`require_exec`, which refuses `substrate-mcp.resource-untracked`), then reads the resource
through the daemon, then sends the mutation:

- `workspace_destroy`: `get_workspace`, then `destroy_with_operation_id`;
- `workspace_file_write`: `get_workspace`, then `write_file_with_operation_id`;
- `workspace_file_delete`: `get_workspace`, then `delete_file_with_operation_id`;
- `workspace_lease_renew`: `get_workspace`, then `renew_lease_with_operation_id`;
- `exec_signal`: `get_exec`, then `signal_with_operation_id`;
- `exec_lease_renew`: `get_exec`, then `renew_lease_with_operation_id`;
- `exec_retire`: `get_exec`, then `retire_with_operation_id`;
- `exec_start`: `get_workspace`, then the command builder's `start`.

`workspace_destroy` and `exec_retire` take the id out of the registry once the resource is
absent, so the same call again is refused `substrate-mcp.resource-untracked` by the registry
check before any call to the daemon. `workspace_create` reads nothing, and the read-only tools
send no mutation. The adapter's own cleanup (`crates/substrate-mcp/src/lib.rs`) uses fresh
operation ids and takes `resource.not-found` as done.

The CLI's `workspace destroy` (0.25.0), `workspace renew`, `session signal` and `exec run` send
by id (`story:commands-that-read-before-a-mutation-replay-their-stored-outcome`).

## Where the requirement comes from

That story's sentence "The MCP adapter's tools with the same shape are checked and listed", and
the adversary pass on it (`review-result:cli-replay-adversary`). Stakeholder: the operator.

## What changes

1. Each of the eight tools sends its mutation by id, without the read through the daemon, with
   the SDK call the CLI uses where one exists.
2. It is decided, and written here, what the registry check answers for a call made again
   after `workspace_destroy` or `exec_retire` took the id out: the stored outcome, or the
   refusal as today with the reason stated in the tool's description.
3. One case per changed tool in the adapter's existing test binary: the call, the resource
   removed, the same call again.

## Acceptance

The list in this story names, for every tool, "replays" with its case or the refusal decided
under 2 with its case, and `cargo xtask check-mcp-boundary` holds unchanged.
