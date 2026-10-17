---
format: aep.planning-md/3
id: story:host-process-logs
kind: story
status: active
title: Bounded per-process log and a logs route
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:39Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:39Z", actor: "human:timo", revision: 3}
---
# Story: bounded per-process log and a logs route

## Context

Processes started in the background write to `nohup.out` or temporary files nobody finds. The log belongs beside the record, bounded, and removed with it.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S7). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-host/src/`: stdout/stderr capture into one bounded file per record under the owner-private state directory, oldest-first discard with a count.
- `crates/substrate-daemon/src/app/`: `GET /v1/processes/{process_id}/logs` bounded tail by stream; retention of terminal records.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-output-is-logged-under-daemon-state`
- `host-process-log-lines-name-their-stream`
- `host-process-log-bound-discards-oldest-and-counts`
- `host-process-logs-route-returns-a-bounded-tail`
- `host-process-log-is-removed-with-its-record`
- `host-process-nothing-is-written-to-the-caller-cwd`
