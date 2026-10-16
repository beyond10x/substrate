---
format: aep.planning-md/3
id: task:release-0-7-16
kind: task
status: draft
title: Release Substrate 0.7.16 with the client CLI wave
revision: 1
---
## Outcome
Publish the 2026-10-16 client CLI wave as Substrate 0.7.16: the `b10x-substrate` command-line client of the daemon socket over `b10x-substrate-sdk`, its ESS CLI presentation binding in `spec/client` with the generated contract package in `spec/generated/cli`, and the ESS pin at 0.60.0. No wire change: the release keeps the development bundle `substrate-wire/0.18.0`. Daemon and MCP OCI images are rebuilt at 0.7.16. The CLI ships as source: `cargo install --path crates/b10x-substrate-cli --locked` from a checkout of the tag.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The package gates of every touched crate on the integrated head, the pull request's full CI gate, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
