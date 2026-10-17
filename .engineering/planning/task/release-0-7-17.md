---
format: aep.planning-md/3
id: task:release-0-7-17
kind: task
status: draft
title: Release Substrate 0.7.17 with the adopt and host-process wave
revision: 1
---
## Outcome
Publish the 2026-10-17 adopt and host-process wave as Substrate 0.7.17: the `workspace.adopt` route (ADR 0035) with development bundle `substrate-wire/0.19.0`, and the ESS domain, store and host groundwork of the host-process family (ADR 0036) with no daemon route yet. Daemon and MCP OCI images are rebuilt at 0.7.17 and the bundle image `b10x-substrate-wire:0.19.0` is published.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. Workspace clippy, `check-licenses` and `check-packages` on the release head, the pull request's full CI gate, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
