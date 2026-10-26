---
format: aep.planning-md/3
id: task:release-0-11-0
kind: task
status: implemented
title: Release Substrate 0.11.0 on ESS 0.69.2 with bundle 0.23.0
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-26T14:51:31Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-26T14:51:31Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-26T15:50:35Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome
Publish Substrate 0.11.0 on ESS 0.69.2: development bundle `substrate-wire/0.23.0` (read-only roots, session-start exec fields, stream bounds) with wave 2026-10-23 and the ESS pin move (`task:ess-pin-moves-to-0-69-2`, pull request 157). Daemon and MCP OCI images are built at 0.11.0; the development bundle is `substrate-wire/0.23.0`.

## Approval
Session b10x-chef-orchestrator, 2026-10-26: move the repository to the newest ESS release, land it by the repository's route, and release per AGENTS.md (class C under the workspace grant; operator, 2026-10-05: "ALWAYS upgrade to most recent ESS").

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The pull request's full CI gate and macOS job, the delegated lane and the project-quota lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` and `Quota-lane:` records, the release workflow, signatures and artifact read-back.
