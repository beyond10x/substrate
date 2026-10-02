---
format: aep.planning-md/3
id: task:release-0-7-10
kind: task
status: active
title: Release Substrate 0.7.10 with bounded live output delivery
relations:
- delivers: story:live-output-stall
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-02T22:51:13Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-02T22:51:13Z", actor: "human:timo", revision: 3}
---
## Outcome
Publish the reviewed live-output-stall correction as Substrate0.7.10, including daemon/MCP OCI artifacts and the unchanged0.17 development contract bundle. Existing source outcome: story:live-output-stall implemented, PR116 merged7d21dbb4. The operator asked to continue shipping the usable integration after the prior release stop; this follows the existing release intent and correction work.

## Scope and verification
Cargo.toml, Cargo.lock, internal exact package-version edges in crates/*/Cargo.toml and xtask/Cargo.toml, CHANGELOG.md. No runtime behavior or immutable bundle changes. Run the full repository gate, exact-tagged-commit delegated lane, required CI, annotated bare version tag, release workflow, signatures and required artifact verification. Capture absence of quota and resource-counter fixtures honestly. Docs publish asynchronously under the repository completion boundary.

## Delivery
Root owns AEP, bot publication, release and artifact verification. Implementor prepares version/changelog only in the retained managed worktree. Preserve private untracked evidence. No existing worker restart or workspace destruction is authorized by this source release task.
