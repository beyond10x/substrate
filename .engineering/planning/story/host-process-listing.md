---
format: aep.planning-md/3
id: story:host-process-listing
kind: story
status: active
title: List host processes with what was observed
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:38Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:38Z", actor: "human:timo", revision: 3}
---
# Story: list host processes with what was observed

## Context

One place to see everything that is running: name, owner, cwd, argv, start, lease, deadline, state, CPU, memory and listening ports.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S6). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-daemon/src/app/`: `GET /v1/processes` with owner, name and state filters and a bounded page.
- `crates/substrate-host/src/`: CPU from `cpu.stat`, `memory_bytes` from `memory.current`, listening sockets matched by inode to cgroup members.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-list-shows-owner-cwd-argv-lease-and-state`
- `host-process-list-filters-by-owner-name-and-state`
- `host-process-list-reports-observed-cpu-and-memory`
- `host-process-list-reports-an-observed-listening-port`
- `host-process-unobservable-measure-is-absent-not-zero`
- `host-process-list-never-shows-env-values`
- `host-process-list-is-scoped-to-the-subject`
