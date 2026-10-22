---
format: aep.planning-md/3
id: story:writable-roots-wire-0-22-0
kind: story
status: active
title: Bundle substrate-wire/0.22.0 publishes writable roots
scope:
- confidence: cited
  path: contracts/substrate-wire/0.22.0
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: cited
  path: scripts/gate.sh
- confidence: cited
  path: xtask/bundle-source/0.22.0
- confidence: cited
  path: xtask/src/bundle.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T11:02:42Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-22T11:02:42Z", actor: "agent:b10x-chef-orchestrator", revision: 4}
---
# Bundle substrate-wire/0.22.0 publishes writable roots

## Outcome

A client can validate a start carrying `writable_roots` against a released bundle. `0.22.0` is an
additive successor of `0.21.0`: the `writable_roots` start field as a closed union of
`{host, mount}` and `{workspace, mount}`, at most 8 entries; the facts `exec.writable-roots.host`
and `exec.writable-roots.git`; the applied-record members (kind, name or workspace id, host path,
mount); and the named refusals of ADR 0038, each bound to a `substrate_wire` constant.

Decision: `adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md`.

## Acceptance

- `cargo xtask check-bundle 0.22.0` passes, and `scripts/gate.sh` runs `check-bundles` through
  `0.22.0` and `check-json` over all twenty-two bundles.
- The wire types in `crates/substrate-wire/src/lib.rs` round-trip every new vector; a start without
  `writable_roots` hashes as it did under `0.21.0`.
- Daemon and SDK advertise `substrate-wire/0.22.0` with its digest.
- Portable runtime vectors: a start naming either kind is refused `unserved` (501) with the name.

## Scope

`crates/substrate-wire/src/lib.rs`, `xtask/bundle-source/0.22.0/`, `contracts/substrate-wire/0.22.0/`,
`scripts/gate.sh`, `xtask/src/bundle.rs`, `crates/substrate-daemon/tests/runtime_vectors.rs`.
Lands before the two kind stories, which use its types.
