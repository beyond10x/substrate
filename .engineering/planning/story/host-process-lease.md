---
format: aep.planning-md/3
id: story:host-process-lease
kind: story
status: implemented
title: Host process leases expire, and keep needs a reason
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:37Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-21T23:37:40Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: host process leases expire, and keep needs a reason

## Context

A forgotten process must end on its own. Exec, session and workspace leases already exist; a host process gets the same TTL bounds and renewal route, plus a `keep` lease that is declared at start with its own reason.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S4). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-wire/src/lib.rs`: the lease union (TTL or keep with `keep_reason`) and its refusals.
- `crates/substrate-daemon/src/app/`: `POST /v1/processes/{process_id}/lease/renew` and the daemon-originated expiry operation.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-lease-expiry-stops-the-tree`
- `host-process-lease-renewal-extends-renew-by`
- `host-process-keep-has-no-ttl-and-records-its-reason`
- `host-process-keep-without-reason-is-refused` (`process.keep-reason-required`)
- `host-process-ttl-out-of-bounds-is-refused` (`process.lease-invalid`)
- `host-process-lease-cannot-become-keep-after-start`
- `host-process-expiry-interrupted-by-crash-completes-on-restart`
