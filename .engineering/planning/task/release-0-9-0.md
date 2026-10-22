---
format: aep.planning-md/3
id: task:release-0-9-0
kind: task
status: implemented
title: Release Substrate 0.9.0 with workdir and several apertures
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T09:53:57Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-22T09:53:57Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-22T09:53:57Z", actor: "agent:b10x-chef-orchestrator", revision: 4}
---
## Outcome
Publish Substrate 0.9.0: development bundle `substrate-wire/0.21.0` adds `workdir` on a start and two to four `apertures` per sandbox, declared read-only roots are mounted by descriptor, and the host reads bubblewrap's info report to its end. Daemon and MCP OCI images are built at 0.9.0; the development bundle is `substrate-wire/0.21.0`.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The pull request's full CI gate and macOS job, the delegated lane at the exact tagged commit (147 cases), an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back.
