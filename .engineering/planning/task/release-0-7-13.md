---
format: aep.planning-md/3
id: task:release-0-7-13
kind: task
status: implemented
title: Release Substrate 0.7.13 with wire 0.18.0 and confinement fixes
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-12T17:11:09Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":4}}}
- {from: "proposed", to: "active", at: "2026-10-12T17:11:09Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":4}}}
- {from: "active", to: "implemented", at: "2026-10-12T17:11:09Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":4}}}
---
## Outcome
Publish the wire 0.18.0 wave as Substrate 0.7.13: exec usage served without the `io` controller (ADR 0032, bundle `substrate-wire/0.18.0`), a single host file admitted read-only (ADR 0033), and the two confinement fixes in the changelog's Security section (the pinned workspace descriptor no longer reaches the confined program; driver descriptors sit above the secret-slot range). Daemon and MCP OCI images are rebuilt at 0.7.13 and the development bundle `0.18.0` is published for the first time.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and `THIRD_PARTY_LICENSES.html` regenerated with cargo-about 0.9.1. The full local gate on the exact release-preparation head, the pull request's full CI gate, the delegated lane at the exact tagged commit (106 delegated cases), an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
