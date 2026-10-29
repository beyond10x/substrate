---
format: aep.planning-md/3
id: story:block-io-and-bandwidth-limits
kind: story
status: draft
title: An exec carries declared block-IO and network bandwidth limits
summary: 'Glovebox wave 4: a design decides io.max and per-aperture rate limits; block IO is observed today but not limited, network has only a byte ceiling.'
owner: substrate
tags:
- glovebox
- resource-bounds
revision: 1
---
# Story: an exec carries declared block-IO and network bandwidth limits

Part of Glovebox (atlas `initiative:glovebox`, `docs/design/glovebox.md`), wave 4.

## Why

- Glovebox requirement 1 names quotas for "CPU, memory, IO, network"
  (`atlas/docs/design/glovebox.md:19-21`); its gap table records "IO and network quotas | partly:
  a byte ceiling per aperture; block IO observed, not limited" (`atlas/docs/design/glovebox.md:72`).
- Block IO is observed, not limited. ADR 0032 publishes `block_io: true` "only when the delegation
  root enables the `io` controller for its children" and makes the counters absent otherwise,
  "never zero-filled" (`adr/0032-exec-usage-without-block-io.md:27-36`); bundle `0.18.0` carries
  that change (`AGENTS.md:84-86`). The limit facts are `processes`, `memory` and `cpu` and nothing
  else (`crates/substrate-wire/src/lib.rs:3933-3937`). The only `io.max` reference in the crates
  is a test of the systemd adapter's enclosing-bound check
  (`crates/substrate-host/src/confinement/systemd.rs:1563-1568`); no exec writes one.
- Network has a total, not a rate. Design 12 adds "one ceiling over both directions, summed"
  per aperture per run (`docs/design/12-aperture-byte-ceiling.md:40-41`), so an operator can say
  "this run may move 100 MiB" but not "at most 10 MiB/s".
- Design 17 lists the exec's existing hard bounds as wall time, cumulative CPU, memory-plus-swap,
  process count and output (`docs/design/17-resource-accounting-and-storage-quotas.md:9-10`);
  bandwidth is not among them.

## What changes

1. **A design document first** (invariant 8). It decides at least:
   - block IO: whether the bound is cgroup v2 `io.max` (bytes and operations per second, per
     device) on the exec cgroup, which device or devices it names, and how it behaves on a
     delegation root without the `io` controller (the systemd-delegated user scope ADR 0032
     describes has none, `adr/0032-exec-usage-without-block-io.md:16-17`). Absent controller means
     an absent fact and a named refusal, never an unenforced limit (invariant 3);
   - network: whether the bound is a rate on each aperture relay, a rate for the whole run, or
     both, and how it composes with design 12's byte ceiling;
   - the capability facts, the start fields, the applied record, and each named refusal;
   - what a limit hit looks like: throttling with an observation, or a refusal that ends the run.
2. After the design is accepted, a successor contract bundle (invariant 6) and the host
   implementation.

## Acceptance

- The design is accepted (as a design document under `docs/design/` or an ADR) before any code.
- A successor bundle publishes the new facts, start fields, applied record and refusals;
  `cargo xtask check-bundles` and `cargo xtask check-json` pass with it in `scripts/gate.sh`.
- Portable lane: each fact is absent and a start naming a limit is refused by name before dispatch.
- Delegated lane, on a root with the `io` controller: a child that writes a large file under a
  declared block-IO limit takes measurably longer than without it, and the applied record states
  the limit. On a root without `io`, the fact is absent and the start is refused by name.
- Delegated lane: a child moving bytes through an aperture under a declared rate stays at or
  under it within a stated tolerance, and the observation reports the bytes moved.

## Blocked on / Depends on

- Blocked on the design above (invariant 8).
- Depends on nothing outside Substrate. Mantle's `story:manifest-declares-the-box` maps its IO and
  network quota fields onto these limits and refuses them until a Substrate release serves them.
