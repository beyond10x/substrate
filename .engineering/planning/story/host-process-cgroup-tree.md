---
format: aep.planning-md/3
id: story:host-process-cgroup-tree
kind: story
status: implemented
title: One cgroup per host process, and stop kills the tree
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:37Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-21T23:37:40Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# Story: one cgroup per host process, and stop kills the tree

## Context

A process that daemonizes or re-parents to PID 1 escapes a process-tree kill. Cgroup membership, set before the first instruction, is what makes a stop complete.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S3). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-host/src/process.rs`: per-process cgroup creation under the delegated root, `CLONE_INTO_CGROUP`, SIGTERM-grace-`cgroup.kill` stop.
- `crates/substrate-daemon/src/app/`: `POST /v1/processes/{process_id}/stop`, the `ended_by` state machine and restart reconciliation.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-child-is-born-in-its-cgroup`
- `host-process-stop-kills-a-daemonized-grandchild`
- `host-process-main-exit-stops-remaining-members`
- `host-process-exit-reports-code-or-signal`
- `host-process-is-never-restarted`
- `host-process-restart-reobserves-a-live-cgroup`
- `host-process-restart-marks-a-missing-cgroup-vanished`
- `host-process-stop-of-unknown-id-is-refused` (`process.not-found`)
