---
format: aep.planning-md/3
id: task:release-0-7-18
kind: task
status: draft
title: Release Substrate 0.7.18 with the macOS host build
revision: 2
---
## Outcome
Publish Substrate 0.7.18: `b10x-substrate-host` compiles on macOS and refuses by name off Linux (`story:macos-host-compiles`), and `rustls-webpki` moves to 0.103.16 for RUSTSEC-2026-0382. Daemon and MCP OCI images are rebuilt at 0.7.18; the development bundle stays `substrate-wire/0.19.0`.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The pull request's full CI gate and macOS job, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back.
