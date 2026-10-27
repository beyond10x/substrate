---
format: aep.planning-md/3
id: story:confinement-consumer-surface
kind: story
status: implemented
title: The consumer surface loom-sdk and takt call
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-contract-on-linux-and-local
- depends_on: story:macos-adapter-on-seatbelt
- depends_on: story:systemd-scope-bounds-provider
- depends_on: story:oci-adapter
- depends_on: story:pod-adapter
- depends_on: story:macos-seatbelt-policy-runs-cargo-test
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: inferred
  path: crates/substrate-host/tests/consumer_surface.rs
- confidence: inferred
  path: scripts/delegated-lane.sh
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":6}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":6}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"test_result":1,"review_outcome":7}}}
---
# The consumer surface loom-sdk and takt call is proved and frozen

ADR 0109 decision 6: loom-sdk and takt consume Substrate only. loom's `SubstrateRunner` embeds
`HostDriver` in process today and checks the OS itself (`/usr/bin/bwrap`, a delegated cgroup;
loom `crates/loom-intake-slice/src/confinement.rs:123,370,3264,3418`); takt carries its own Seatbelt
profiles, an `unshare` path and a bubblewrap promotion build (takt `crates/takt/src/confinement.rs`,
`seatbelt.rs`, `forge.rs`, `promotion.rs`).

`story:confinement-contract-on-linux-and-local` defines and exports the `confinement` module. This
story does not change it. It adds what proves that module is enough for both consumers and what
stops it from drifting: a consumer-written test, a committed list of the public items, and the
documentation a consumer reads. It runs after the five adapter stories, so the list it freezes is
the surface they implement. Their later hops (loom L1, takt T1) are then pin bumps plus deletions.

## Acceptance

- `crates/substrate-host/tests/consumer_surface.rs` (new) is written as a consumer would: it imports
  only `b10x_substrate_host::confinement`, and covers loom's three needs (a toolchain root
  read-only, the workspace with `target/` writable, a working directory beneath it) and takt's
  promotion build (an install root outside the clone writable through a declared writable root, a
  `--version` run of the result), each choosing its adapter by reading `served_families` and
  `facts` and never by name or OS. It runs on `linux` in the delegated lane, where every need
  runs. In the portable lane it runs on `local`: the needs `local` serves run (a working directory
  beneath the workspace, a `--version` run), and each need that names a filesystem view or a
  writable root is asserted refused `confinement.family-unserved` naming `filesystem_view`, read
  from the facts before the call. Each lane's output names its case count.
- A committed list of the module's public items (`crates/substrate-host/tests/confinement-public-items.txt`,
  new) is checked by a test that fails when a public item is added, removed or changes signature
  without the list; the test fails on a tree where one item is renamed.
- The public site gains a page on choosing a profile and reading facts, listed in the page
  allowlist, and `cargo xtask check-docs` passes; `README.md` names the module.
- An `aep:adversary` pass is recorded as a `review-result` that `reviews:` this story, and each of
  its findings has a `review_outcome` record: `fixed` in this story, or `escalated` with a filed
  story id.

## Scope

`crates/substrate-host/tests/consumer_surface.rs` (new),
`crates/substrate-host/tests/confinement-public-items.txt` (new), the public docs page and its
allowlist entry, `README.md`, `scripts/delegated-lane.sh` (the new test in the delegated lane),
`CHANGELOG.md` (one line under `## [Unreleased]`; the other units of its wave add theirs, and the
merge into the integration branch keeps every line). Not `crates/substrate-host/src/`.

## Depends on

`story:macos-adapter-on-seatbelt`, `story:macos-seatbelt-policy-runs-cargo-test`,
`story:systemd-scope-bounds-provider`, `story:oci-adapter`, `story:pod-adapter` (the surface they
implement, `confinement/macos.rs` included), and through them
`story:confinement-contract-on-linux-and-local`.
