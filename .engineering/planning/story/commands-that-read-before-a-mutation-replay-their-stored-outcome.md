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
- confidence: inferred
  path: crates/b10x-substrate-sdk/src/lib.rs
revision: 2
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

