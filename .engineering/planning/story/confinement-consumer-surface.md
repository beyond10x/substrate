---
format: aep.planning-md/3
id: story:confinement-consumer-surface
kind: story
status: draft
title: The consumer surface loom-sdk and takt call
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-contract-on-linux-and-local
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/lib.rs
- confidence: inferred
  path: crates/substrate-host/tests/consumer_surface.rs
revision: 2
---
# The consumer surface loom-sdk and takt call

ADR 0109 decision 6: loom-sdk and takt consume Substrate only. loom's `SubstrateRunner` embeds
`HostDriver` in process today and checks the OS itself (`/usr/bin/bwrap`, a delegated cgroup;
loom `crates/loom-intake-slice/src/confinement.rs:123,370,3264,3418`); takt carries its own Seatbelt
profiles, an `unshare` path and a bubblewrap promotion build (takt `crates/takt/src/confinement.rs`,
`seatbelt.rs`, `forge.rs`, `promotion.rs`). This story fixes the one surface both will call so that
their later hops (loom L1, takt T1) are pin bumps plus deletions.

## Acceptance

- `b10x-substrate-host` exports one module, `confinement`, whose public items are the profile, the
  adapter request (`local { reason }` or a named confined adapter), `open`, the facts with
  `served_families`, the refusal type with its code and the family or fact named, and run handles
  for run, watch, output and wait. Nothing a client needs is reached through `HostDriver` or a
  platform `cfg`.
- `crates/substrate-host/tests/consumer_surface.rs` is written as a consumer would: it imports only
  `b10x_substrate_host::confinement`, and covers loom's three needs (a toolchain root read-only, the
  workspace with `target/` writable, a working directory beneath it) and takt's promotion build
  (an install root outside the clone writable through a declared writable root, a `--version` run
  of the result), each choosing its adapter by reading `served_families` and never by name or OS; it
  runs on `local` in the portable lane and on `linux` in the delegated lane.
- A `cargo public-api`-style snapshot of the module (or an equivalent committed list of its public
  items, checked by a test) fails when a public item changes without the snapshot.
- The public site gains a page on choosing a profile and reading facts (`cargo xtask check-docs`
  passes), and README names the surface.

## Scope

`crates/substrate-host/src/confinement/mod.rs` (the exported surface), `crates/substrate-host/src/lib.rs`,
`crates/substrate-host/tests/consumer_surface.rs` (new), a public-items snapshot file (new), the
public docs page and its allowlist entry, `README.md`, `CHANGELOG.md`.

## Depends on

`story:confinement-contract-on-linux-and-local`. Runs after the wave holding the adapters, so the
surface it fixes is the one they implement.
