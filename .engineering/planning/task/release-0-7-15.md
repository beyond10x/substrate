---
format: aep.planning-md/3
id: task:release-0-7-15
kind: task
status: draft
title: Release Substrate 0.7.15 with the containment-and-listing wave
revision: 1
---
## Outcome
Publish the 2026-10-14 containment-and-listing wave as Substrate 0.7.15: a claimed session's unproven containment kill retried by a tracked containment step, failed-upgrade containment moved out of a detached task, directory listings that omit FIFOs, sockets and non-UTF-8 names instead of refusing, workspace file reads that no longer block on a FIFO, and runtime-vector cgroup teardown failures that name what the group held. No wire change: the release keeps the development bundle `substrate-wire/0.18.0`. Daemon and MCP OCI images are rebuilt at 0.7.15.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. The package gates of every touched crate on the integrated head, the pull request's full CI gate, the delegated lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` record, the release workflow, signatures and artifact read-back. Documentation publishes asynchronously.
