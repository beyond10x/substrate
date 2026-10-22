---
format: aep.planning-md/3
id: task:release-0-10-0
kind: task
status: implemented
title: Release Substrate 0.10.0 with writable roots
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T23:00:38Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-22T23:00:38Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-22T23:00:39Z", actor: "agent:b10x-chef-orchestrator", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome
Publish Substrate 0.10.0: development bundle `substrate-wire/0.22.0` adds `writable_roots` on a start, each an operator-declared host directory or a Git checkout workspace of the same subject (ADR 0038). Daemon and MCP OCI images are built at 0.10.0; the development bundle is `substrate-wire/0.22.0`.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The pull request's full CI gate and macOS job, the delegated lane at the exact tagged commit (167 cases), an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back.
