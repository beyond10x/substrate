---
format: aep.planning-md/3
id: task:release-0-15-0
kind: task
status: draft
title: Release 0.15.0
relations:
- delivers: epic:confinement-contract-family
revision: 1
---
## Outcome
Publish Substrate 0.15.0, the release of `wave/adapter-io`: design 32's piped standard input (`stdin_pipe`) and live output delivery (`output_live`) on the confinement adapters, design 33's host execution AppArmor profile name as a deployment parameter, the `pod` fix that keeps a run's NetworkPolicy until the API shows its container terminated when its Pod was force-deleted, and ADR 0047 accepted. Also released: everything else under `[Unreleased]` since 0.14.0 (CHANGELOG.md `[0.15.0]`), among it the host-process lifecycle events of ADR 0036 S12. The development bundle is `substrate-wire/0.24.0`, which the release workflow already pins.

## Approval
Session b10x-chef-orchestrator, 2026-10-29, under its standing release grant.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, the generated release notes page, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. Minor, not patch: breaking `substrate-host` public API changes (two new `Fact` variants, `FACT_ORDER` length, two new `ConfinementProfile` fields) and a new contract bundle. Verified by the pull request's full CI gate, the delegated lane and the project-quota lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` and `Quota-lane:` records, the release workflow, signatures and artifact read-back.
