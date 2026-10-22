---
format: aep.planning-md/3
id: task:release-0-8-0
kind: task
status: implemented
title: Release Substrate 0.8.0 with the host-process routes
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T01:00:27Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-10-22T01:00:27Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-10-22T01:00:27Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome
Publish Substrate 0.8.0: the daemon serves host processes over seven `/v1/processes` routes in development bundle `substrate-wire/0.20.0`, exec binds an adopted workspace by a no-symlink descriptor, and the ESS pin moves to 0.67.0. Daemon and MCP OCI images are built at 0.8.0; the development bundle is `substrate-wire/0.20.0`.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The pull request's full CI gate and macOS job, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back.
