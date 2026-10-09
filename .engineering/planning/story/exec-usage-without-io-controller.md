---
format: aep.planning-md/3
id: story:exec-usage-without-io-controller
kind: story
status: draft
title: Exec usage is served on a host without the io controller
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: exec usage is served on a host without the io controller

## Problem

`probe_resource_usage` (`crates/substrate-host/src/probe.rs`) requires `io.stat` beside the CPU,
memory and pids counters. A systemd-delegated user scope carries `cpu memory pids` and no `io`, so
the whole `exec.resource-usage` fact is withheld and an exec requesting `resource-usage` is refused
`exec.metrics-unserved`, although every counter but block I/O is available.

## Contract

Released bundles through `0.17.0` fix `exec.resource-usage.block_io` as `const: true`, and
`ResourceUsage` requires `io_read_bytes` and `io_write_bytes`. Serving usage without I/O is a wire
change: a successor bundle in which `block_io` is a boolean and the two I/O counters are absent when
it is `false`, with its design record before code (invariants 6 and 8). Clients that read the fact
are affected, so the change waits for an explicit decision.

## Acceptance

- On a delegation root with `cpu memory pids` and no `io`, the capability snapshot publishes
  `exec.resource-usage` with `block_io: false` and every other member `true`.
- An exec that requests `resource-usage` there is dispatched and its usage record carries CPU,
  memory and pids counters with no I/O counters.
- On a delegation root with `io`, the fact and the record are unchanged from `0.17.0`.
- The successor bundle is rendered from `xtask/bundle-source/`, checked by `cargo xtask
  check-bundles` in `scripts/gate.sh`, and advertised by the daemon and SDK.
