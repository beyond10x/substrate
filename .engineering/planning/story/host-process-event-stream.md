---
format: aep.planning-md/3
id: story:host-process-event-stream
kind: story
status: draft
title: Per-process event stream
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
- depends_on: story:host-process-logs
revision: 1
---
# Story: per-process event stream

## Context

Sessions wait on log lines and files in polling loops that time out or never match. A stream of a process's lifecycle and output lets a caller's watcher read what happened instead of polling; it ships before any such watcher.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S12). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-wire/src/lib.rs`: the `process.*` variants of the closed event union, the per-process stream item union (event, line, gap, terminal), `MAX_PROCESS_EVENT_LINE_BYTES`, `MAX_PROCESS_EVENT_READERS`.
- `crates/substrate-daemon/src/app/`: lifecycle events into the existing event log; `GET /v1/processes/{process_id}/events` WebSocket reading the S7 log with a per-process cursor.
- `crates/b10x-substrate-sdk/src/`: the per-process stream call beside `event_stream`.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-lifecycle-appears-in-the-global-event-log`
- `host-process-output-lines-never-enter-the-global-event-log`
- `host-process-stream-interleaves-events-and-lines-in-order`
- `host-process-stream-ends-with-exactly-one-terminal-item`
- `host-process-stream-terminal-item-names-ended-by`
- `host-process-stream-resumes-from-its-cursor`
- `host-process-stream-reports-a-gap-for-discarded-log-bytes`
- `host-process-long-line-is-truncated-with-its-length`
- `host-process-non-utf8-line-is-marked-lossy`
- `host-process-slow-reader-never-blocks-the-program`
- `host-process-stream-on-an-ended-record-replays-and-closes`
- `host-process-stream-readers-bound-is-refused` (`process.events-readers-exceeded`)
