---
format: aep.planning-md/1
id: story:materialize-connector-git-sources
kind: story
status: implemented
title: Materialize connector-authorized Git sources
summary: Serve the existing Git workspace source through a bounded Connector byte-plane authority.
relations:
- decomposes: epic:byte-plane-completion
- depends_on: epic:remote-serving
scope:
- confidence: cited
  path: README.md
- confidence: cited
  path: contracts/substrate-wire/0.16.0
- confidence: cited
  path: crates/b10x-substrate-sdk
- confidence: cited
  path: crates/substrate-daemon
- confidence: cited
  path: crates/substrate-daemon/src/app/routes.rs
- confidence: cited
  path: crates/substrate-host
- confidence: inferred
  path: crates/substrate-sdk
- confidence: cited
  path: crates/substrate-wire
- confidence: inferred
  path: docs/design/21-connector-authorized-git-sources.md
- confidence: cited
  path: scripts/gate.sh
- confidence: cited
  path: website/docs/reference/contract.md
- confidence: inferred
  path: xtask/bundle-source
revision: 8
---
# Story: Materialize connector-authorized Git sources

## Outcome

Substrate creates one normal Git working tree at an exact commit through a configured Connector source, reports the source truthfully, and exposes bounded source-relative observations.

## Acceptance

- `docs/design/21-connector-authorized-git-sources.md` fixes authority, destination, limits, recovery, refusal, and observation semantics before runtime code.
- Git source input names a configured source and opaque locator, plus provider reference, exact commit, and depth; secret authority is header-only.
- Checkout is atomic, restart-reconcilable, exact-commit verified, and has no proxy, redirect, LFS, submodule, hook, helper, or arbitrary destination escape.
- File APIs hide Git control data while confined terminals retain normal Git behavior.
- Bounded change-set and baseline-file observations avoid whole-tree content reads.
- `workspace.git` is advertised only after local mechanism and configuration probes pass.

## Scope

- `docs/design/21-connector-authorized-git-sources.md` — inferred
- `crates/substrate-wire` — cited
- `crates/substrate-host` — cited
- `crates/substrate-daemon` — cited
- `crates/substrate-sdk` — inferred
- `xtask/bundle-source` — inferred

## Implementation evidence

- `docs/design/21-connector-authorized-git-sources.md` fixes the source aperture, transient authority, exact-commit, staging/recovery, private baseline and refusal semantics.
- `crates/substrate-host/src/git.rs` implements bounded no-proxy/no-redirect/no-filter fetch, exact commit checkout, fsync plus no-replace installation, private baseline reconciliation, bounded baseline reads and path-sorted bounded change sets.
- `crates/substrate-host/src/fs.rs` returns not-found for direct `.git` reads/writes/deletes and omits it from directory/tree observations while terminals retain the physical working tree.
- `crates/substrate-daemon` and `crates/b10x-substrate-sdk` serve and consume the typed v2 Git baseline/change-set routes; source authority remains a one-request header.
- `contracts/substrate-wire/0.16.0` is generated from `xtask/bundle-source/0.16.0`, succeeds 0.15.0 additively, and is advertised with its exact inner digest.
