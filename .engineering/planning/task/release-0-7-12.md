---
format: aep.planning-md/3
id: task:release-0-7-12
kind: task
status: draft
title: Release Substrate 0.7.12 with host-run gate changes
revision: 1
---
## Outcome
Publish the unreleased `main` work since 0.7.11 as Substrate 0.7.12: the configurable exec CPU ceiling, aperture state outside the workspace root, and Git in a linked worktree through a read-only root (PR 122). Daemon and MCP OCI images are rebuilt at 0.7.12; the 0.17.0 development contract bundle is unchanged and reused byte-identically.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and `THIRD_PARTY_LICENSES.html` regenerated with cargo-about 0.9.1. No bundle change. Package gates for the touched crates on the exact commit, the pull request's full CI gate, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
