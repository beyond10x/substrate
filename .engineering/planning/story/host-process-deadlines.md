---
format: aep.planning-md/3
id: story:host-process-deadlines
kind: story
status: active
title: Hard timeout and expires-at deadlines
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
- depends_on: story:host-process-cgroup-tree
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:39Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:39Z", actor: "human:timo", revision: 3}
---
# Story: hard timeout and expires-at deadlines

## Context

A short command is waited on for hours when nothing ends it. A hard deadline, separate from the renewable lease, ends it and the record says which limit did.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S11). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-wire/src/lib.rs`: `timeout_ms`, `expires_at`, `MAX_PROCESS_TIMEOUT_MS`, `ended_by` values `timeout` and `expires-at`, the two refusals.
- `crates/substrate-daemon/src/app/`: the durable effective deadline, its daemon-originated stop operation and its enforcement on reconciliation.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-timeout-kills-at-the-deadline`
- `host-process-expires-at-kills-at-the-instant`
- `host-process-earlier-of-timeout-and-expires-at-wins`
- `host-process-deadline-is-not-extended-by-lease-renewal`
- `host-process-keep-lease-still-ends-at-its-timeout`
- `host-process-ended-by-names-the-first-cause-only`
- `host-process-deadline-passed-during-downtime-stops-on-restart`
- `host-process-invalid-timeout-is-refused` (`process.timeout-invalid`)
- `host-process-invalid-expires-at-is-refused` (`process.expires-at-invalid`)
