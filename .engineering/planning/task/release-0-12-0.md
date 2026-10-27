---
format: aep.planning-md/3
id: task:release-0-12-0
kind: task
status: active
title: Release Substrate 0.12.0 with the ADR 0109 confinement contract
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T06:57:27Z", actor: "human:timo", revision: 2, executor: "agent:substrate-adr-0109"}
- {from: "proposed", to: "active", at: "2026-10-27T06:57:28Z", actor: "human:timo", revision: 3, executor: "agent:substrate-adr-0109"}
---
## Outcome
Publish Substrate 0.12.0 with atlas ADR 0109: the `substrate_host::confinement` contract with its `local`, `linux`, `macos`, `systemd`, `oci` and `pod` adapters, the frozen consumer surface and `b10x-substrate confine run`. Daemon and MCP OCI images are built at 0.12.0; the development bundle stays `substrate-wire/0.23.0` (no wire change).

## Approval
Session b10x-chef-orchestrator, 2026-10-27: implement atlas ADR 0109 in Substrate and release it through the repository's own process (under the delivery grant). The release preparation lands in the same pull request as the implementation, at the operator's request for one pull request.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. Minor, not patch: the confinement module and the `confine run` verb are new public surface. Verified by the pull request's full CI gate and macOS job, the delegated lane and the project-quota lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` and `Quota-lane:` records, the release workflow, signatures and artifact read-back.
