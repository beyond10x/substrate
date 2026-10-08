---
format: aep.planning-md/3
id: task:release-0-7-11
kind: task
status: active
title: Release Substrate 0.7.11 with the docs route list
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T17:40:11Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-08T17:40:12Z", actor: "human:timo", revision: 3}
---
## Outcome
Publish the unreleased `main` work since 0.7.10 as Substrate 0.7.11: the documentation build's `.well-known/b10x-routes.json` route list (PR 119) and the recorded 0.7.10 artifact evidence (PR 118). Daemon and MCP OCI images are rebuilt at 0.7.11; the 0.17.0 development contract bundle is unchanged and reused byte-identically.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and `THIRD_PARTY_LICENSES.html` regenerated with cargo-about 0.9.1 (eight workspace package version labels, no third-party change). No runtime behaviour or bundle change. Package gates for the touched crates on the exact commit, the pull request's full CI gate, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
