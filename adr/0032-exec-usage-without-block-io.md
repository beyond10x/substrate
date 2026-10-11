---
status: accepted
date: 2026-10-11
---

# ADR 0032: exec usage is served without block I/O

## Context

[ADR 0021](0021-execution-metrics-are-explicit-exact-observations.md) made execution resource
usage an explicit, exact observation, and bundles `0.11.0` through `0.17.0` publish it as one
all-or-nothing capability fact: `exec.resource-usage` declares `block_io` as `const: true`, and
`ResourceUsage` requires `io_read_bytes` and `io_write_bytes`. The host withholds the whole fact
unless the delegation root carries `io.stat` beside the CPU, memory and process counters.

A systemd-delegated user scope carries the `cpu memory pids` controllers and no `io`. On such a
root every counter but block I/O is exactly observable, yet the fact is absent and every exec that
requests `measurements: ["resource-usage"]` is refused `exec.metrics-unserved`. The block I/O
counters cannot be served there; reporting them as zero would be the optimistic fact invariant 3
forbids, because zero is a claim that no block I/O happened.

## Decision

Cut successor bundle `0.18.0` from `0.17.0`.

- `exec.resource-usage.block_io` becomes a boolean. Every other member stays `const: true`.
- `io_read_bytes` and `io_write_bytes` become optional in the observed `exec-usage` record.
- When the capability snapshot publishes `block_io: false`, both counters are **absent** from every
  usage record and every metrics sample: the exec resource, `metrics.get` and the
  `metrics.stream` frames. They are never zero-filled.
- When it publishes `block_io: true`, both counters are present, exactly as in `0.17.0`.

The host publishes `exec.resource-usage` once the CPU, memory and process counters are present on
the delegation root, independently of block I/O. It publishes `block_io: true` only when the
delegation root enables the `io` controller for its children (`io` in its
`cgroup.subtree_control`), which is what puts `io.stat` in each exec cgroup. An exec cgroup reads
`io.stat` only when its own `cgroup.controllers` lists `io`; otherwise the counters are absent. A
listed controller whose `io.stat` cannot be read or parsed remains the existing named
`exec.metrics-read-failed` failure, never a silent absence.

The daemon and the Rust SDK advertise `substrate-wire/0.18.0` with the SHA-256 of its inner
`bundle.json`. The session capability document is unchanged in shape, so it continues to name
`substrate-wire/0.17.0`, the bundle whose result schema it conforms to.

## Affected surface

- **Capabilities.** `exec.resource-usage` is now published on a delegation root without `io`, with
  `block_io: false`. `metrics.stream` follows `exec.resource-usage` as before.
- **Refusals.** `exec.metrics-unserved` is no longer returned merely because the `io` controller is
  missing; it remains the refusal for a host without the CPU, memory or process counters. No
  refusal code is added or removed.
- **Observations.** The observed `exec-usage` record, the exec resource, `metrics.get` and
  `metrics.stream` frames may omit both block I/O counters, always together and only under
  `block_io: false`.
- **Events and routes.** No event kind, route, input schema or persisted ledger format changes. The
  successor adds no route and preserves all 36 of `0.17.0`'s.

## Compatibility

For producers this is an additive relaxation: every `0.17.0` document still validates against
`0.18.0`. A consumer that required the block I/O counters must tolerate their absence and read
`block_io` from the snapshot that bound the exec. Harness, the one in-collection client, pins the
wire crate by Git revision and moves at its next pin; it is not changed here. Every released bundle
directory stays byte-for-byte frozen.

## Verification

Host tests prove the fact on a delegation root without `io` (`block_io: false`, every other member
`true`), a usage record without `io.stat` that omits both keys, and an unchanged record where `io`
is present. The rendered `0.18.0` bundle is checked by `cargo xtask check-bundles` in the gate, and
the runtime vectors, delegated lane and contract vectors remain required. Cgroup confinement stays
outside the ESS accepted-operation projection (`spec/coverage.md`), so no specification model
changes.
