---
format: aep.planning-md/3
id: task:release-0-13-0
kind: task
status: implemented
title: Release Substrate 0.13.0 with the ADR 0109 follow-ups
relations:
- delivers: epic:confinement-contract-family
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T10:40:23Z", actor: "human:timo", revision: 2, executor: "agent:substrate-adr-0109"}
- {from: "proposed", to: "active", at: "2026-10-27T10:40:24Z", actor: "human:timo", revision: 3, executor: "agent:substrate-adr-0109"}
- {from: "active", to: "implemented", at: "2026-10-27T13:17:25Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome
Publish Substrate 0.13.0 with the ADR 0109 follow-ups: a confined `cargo build` on `macos` links with no linker set by the caller; `b10x-substrate confine run` removes the groups a SIGKILL'd run left; the CI pod lane runs the pod adversary cases with a guard that refuses a skipped cluster case; the public-items freeze covers the field types of re-exported types; an egress aperture's listener answers nothing after a refused install. Daemon and MCP OCI images are built at 0.13.0; the development bundle stays `substrate-wire/0.23.0` (no wire change).

## Approval
Session b10x-chef-orchestrator, 2026-10-27: continue the follow-up stories on `integration/adr-0109-followups` to the second pull request and its release under the same approval as 0.12.0. Downstream repositories develop against that branch and wait for this release.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. Minor, not patch: a new refusal (`exec.sandbox-developer-tools-unresolved`), new environment the `macos` adapter gives a start, and `left_groups` in the `confine run` JSON document. Verified by the pull request's full CI gate, pod lane and macOS job, the delegated lane and the project-quota lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` and `Quota-lane:` records, the release workflow, signatures and artifact read-back.
