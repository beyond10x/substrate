---
format: aep.planning-md/3
id: story:host-process-stop-by-owner
kind: story
status: active
title: Stop every host process of an owner session
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:38Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:38Z", actor: "human:timo", revision: 3}
---
# Story: stop every host process of an owner session

## Context

A caller's session-end handler needs one call that ends everything the session started, without reaching another subject's records.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S5). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-daemon/src/app/`: `POST /v1/processes/stop` with `owner_session` and `except_keep`, one operation per process.
- `crates/b10x-substrate-sdk/src/lib.rs`: the stop-by-owner call.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-stop-by-owner-stops-every-record-of-that-session`
- `host-process-stop-by-owner-records-one-operation-each`
- `host-process-stop-by-owner-never-reaches-another-subject`
- `host-process-stop-by-owner-except-keep-spares-keep-leases`
- `host-process-stop-by-owner-with-no-records-is-empty-not-refused`
