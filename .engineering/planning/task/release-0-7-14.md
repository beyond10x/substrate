---
format: aep.planning-md/3
id: task:release-0-7-14
kind: task
status: implemented
title: Release Substrate 0.7.14 with the review-fixes wave
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T17:00:32Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":4}}}
- {from: "proposed", to: "active", at: "2026-10-14T17:00:32Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":4}}}
- {from: "active", to: "implemented", at: "2026-10-14T17:00:32Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":4}}}
---
## Outcome
Publish the 2026-10-14 review-fixes wave as Substrate 0.7.14: the metadata-first bubblewrap recheck at exec admission, the duplicate `events` index dropped, line-patch edits applied in line order, the delegated lane's daemon path read from cargo's build output, the attachment permit keyed by a typed session id, and the design 10 and 10a citations corrected. No wire change: the release keeps the development bundle `substrate-wire/0.18.0`. Daemon and MCP OCI images are rebuilt at 0.7.14.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html` (checked against cargo-about 0.9.1 by `cargo xtask check-licenses`). The package gates of every touched crate on the integrated head, the pull request's full CI gate, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
