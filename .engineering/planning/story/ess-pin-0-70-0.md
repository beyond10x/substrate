---
format: aep.planning-md/3
id: story:ess-pin-0-70-0
kind: story
status: active
title: The ESS pin moves to 0.70.0, wire bytes unchanged
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
scope:
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-wire/src/generated
- confidence: inferred
  path: spec/README.md
- confidence: inferred
  path: spec/client/ess-inputs.yaml
- confidence: inferred
  path: spec/coverage.md
- confidence: inferred
  path: spec/ess-inputs.yaml
- confidence: inferred
  path: spec/generated/cli
- confidence: inferred
  path: spec/host-processes-suite.json
- confidence: inferred
  path: spec/host-processes/README.md
- confidence: inferred
  path: spec/host-processes/ess-inputs.yaml
- confidence: inferred
  path: spec/operations-suite.json
- confidence: inferred
  path: spec/sessions/ess-inputs.yaml
- confidence: inferred
  path: spec/workspaces-suite.json
- confidence: inferred
  path: spec/workspaces/README.md
- confidence: inferred
  path: spec/workspaces/ess-inputs.yaml
- confidence: inferred
  path: xtask/src/ess.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":1}}}
---
# The ESS pin moves to 0.70.0

The collection's rule is to use the newest ESS release everywhere; ESS 0.70.0 was released
2026-10-26 (`gh release list -R beyond10x/ess`), and substrate pins 0.69.2 (`spec/ess-inputs.yaml`,
`.github/workflows/gate.yml` `ESS_VERSION`). The new domain of
`story:confinement-domain-specification` is written against the newest release, so the pin moves
first, on its own, and the domain story starts from a tree whose existing suites are already
regenerated.

## Acceptance

- Every `requires: ess …` line under `spec/` and `spec/README.md` name 0.70.0, and
  `.github/workflows/gate.yml` sets `ESS_VERSION` 0.70.0 with `ESS_SHA256` equal to the
  `sha256sum` of the downloaded release archive (the digest `gh release view 0.70.0 -R
  beyond10x/ess --json assets` reports).
- `grep -rn '0\.69\.2' spec xtask .github` prints nothing: the install message in
  `xtask/src/ess.rs` and the facts stated "under ESS 0.69.2" in `spec/coverage.md`,
  `spec/host-processes/README.md` and `spec/workspaces/README.md` name 0.70.0, each re-checked
  against 0.70.0's output rather than renamed.
- `cargo xtask check-ess`, run with the 0.70.0 binary, exits 0: every committed suite and generated
  file equals what 0.70.0 regenerates.
- Wire bytes unchanged: `git diff --stat` of `contracts/` against the story's base is empty, and
  `cargo xtask check-bundles 0.5.0 … 0.23.0` passes. Any change under
  `crates/substrate-wire/src/generated/` is generator output only, and the bundle digest the wire
  crate advertises (`crates/substrate-wire/src/lib.rs`) is unchanged.
- `bash scripts/gate.sh` exits 0.

## Scope

`spec/ess-inputs.yaml`, `spec/*/ess-inputs.yaml`, `spec/README.md`, `spec/coverage.md`,
`spec/host-processes/README.md`, `spec/workspaces/README.md`, `xtask/src/ess.rs`,
`.github/workflows/gate.yml`,
regenerated `spec/*-suite.json`, `spec/generated/cli/`, `crates/substrate-wire/src/generated/*.rs`,
`CHANGELOG.md` (one line under `## [Unreleased]`).

## Not in this story

The confinement domain; any adapter code.
