---
format: aep.planning-md/3
id: task:release-0-14-0
kind: task
status: active
title: Release Substrate 0.14.0 with design 29
relations:
- delivers: epic:confinement-contract-family
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T16:14:09Z", actor: "human:timo", revision: 2, executor: "agent:substrate-adr-0109"}
- {from: "proposed", to: "active", at: "2026-10-27T16:14:09Z", actor: "human:timo", revision: 3, executor: "agent:substrate-adr-0109"}
---
## Outcome
Publish Substrate 0.14.0 with design 29: the `linux` adapter, and `systemd` around it, serves `workspace_access` `ReadOnly` and `Scoped` (writable subtrees over a read-only workspace, `.git` and registered submodules refused as subtrees), and the confinement profile's `output_retention` keeps the head, the tail or both within one bound. Also released: the Docusaurus public site and the signed prebuilt daemon and CLI binaries attached to the GitHub release. The development bundle stays `substrate-wire/0.23.0` (no wire change).

## Approval
Session b10x-chef-orchestrator, 2026-10-27: go on with the release record and the two design 29 stories, and send the next tag with the `verify-release` output.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, the generated release notes page, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. Minor, not patch: two new confinement profile fields, four new refusals (`exec.workspace-access-invalid`, `confinement.writable-subtree-git`, `exec.workspace-scoped-write-unserved`, `confinement.output-retention-unserved`) and the `stdout_kept` / `stderr_kept` observations. Verified by the pull request's full CI gate, pod lane and macOS job, the delegated lane and the project-quota lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:` and `Quota-lane:` records, the release workflow, signatures and artifact read-back.
