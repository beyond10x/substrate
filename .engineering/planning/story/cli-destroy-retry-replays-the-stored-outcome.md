---
format: aep.planning-md/3
id: story:cli-destroy-retry-replays-the-stored-outcome
kind: story
status: draft
title: A retried workspace destroy through the CLI replays the stored outcome
summary: The CLI looks the workspace up before it submits a destroy, so a retry with the same operation id fails resource.not-found where the daemon replays 200
tags:
- defect
revision: 1
---
# Story: a retried workspace destroy through the CLI replays the stored outcome

## Outcome

`b10x-substrate workspace destroy <workspace> --operation-id <id>` run a second time with the
same operation id exits 0 and prints the stored result, as the daemon's own replay does.

## What was seen

Release 0.24.0: the first call exited 0. The same call again exited 1 with `resource.not-found`.
The same `DELETE` replayed against the daemon returned 200 and the stored result. Read on `main`
at `e11fcc988`: `crates/b10x-substrate-cli/src/lib.rs`, `WorkspaceCommand::Destroy`, calls
`client.get_workspace` before `destroy_with_operation_id`.

## What changes

The CLI submits the destroy by workspace id and operation id without reading the workspace
first. Every other CLI command that takes `--operation-id` and reads its resource before the
mutation is checked for the same order; lease renewal has that shape.

## Design

No contract changes: design 16 (idempotent operations) already states the replay.

## Acceptance

A CLI case against the shipped daemon: destroy with an operation id, then the same command
again; both exit 0 and print the same result. Red before the change.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 1, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
