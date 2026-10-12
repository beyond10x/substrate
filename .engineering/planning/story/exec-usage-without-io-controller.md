---
format: aep.planning-md/3
id: story:exec-usage-without-io-controller
kind: story
status: implemented
title: Exec usage is served on a host without the io controller
relations:
- decomposes: epic:host-run-gates
scope:
- confidence: cited
  path: adr/0032-exec-usage-without-block-io.md
- confidence: cited
  path: contracts/substrate-wire/0.18.0
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-wire
- confidence: cited
  path: scripts/gate.sh
- confidence: cited
  path: xtask/bundle-source/0.18.0
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-10-11T19:53:37Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-11T19:54:15Z", actor: "human:timo", revision: 11}
- {from: "active", to: "implemented", at: "2026-10-12T17:11:09Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"test_result":1}}}
---
# Story: exec usage is served on a host without the io controller

## Problem

`probe_resource_usage` (`crates/substrate-host/src/probe.rs`) requires `io.stat` beside the CPU,
memory and pids counters. A systemd-delegated user scope carries `cpu memory pids` and no `io`, so
the whole `exec.resource-usage` fact is withheld and an exec requesting `resource-usage` is refused
`exec.metrics-unserved`, although every counter but block I/O is available.

## Contract

Released bundles through `0.17.0` fix `exec.resource-usage.block_io` as `const: true`, and
`ResourceUsage` requires `io_read_bytes` and `io_write_bytes`. Decided (operator, 2026-10-11):
successor bundle `0.18.0`, with ADR 0032 written before code.

- `exec.resource-usage.block_io` is a boolean.
- `io_read_bytes` and `io_write_bytes` are optional in `ResourceUsage`; when the snapshot says
  `block_io: false` they are absent from every usage record and metrics sample, never zero.
- The daemon and SDK advertise `substrate-wire/0.18.0` with its bundle digest.
- Harness moves at its next pin; it is not changed here.

## Specification

Decision Q3 of `decision-blocker:host-run-gates-contract-route`: cgroup enforcement stays outside
the ESS projection (`spec/coverage.md`); the story is proved by host tests, the runtime vectors and
the delegated lane. `cargo xtask check-ess` still validates the specification with the newest `ess`.

## Acceptance

- On a delegation root with `cpu memory pids` and no `io`, the capability snapshot publishes
  `exec.resource-usage` with `block_io: false` and every other member `true`.
- An exec that requests `resource-usage` there is dispatched and its usage record carries CPU,
  memory and pids counters with no I/O counters.
- On a delegation root with `io`, the fact and the record are unchanged from `0.17.0`.
- The successor bundle is rendered from `xtask/bundle-source/0.18.0/`, checked by `cargo xtask
  check-bundles` in `scripts/gate.sh`, and advertised by the daemon and SDK.
