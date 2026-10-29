---
format: aep.planning-md/3
id: task:release-0-16-0
kind: task
status: active
title: Release 0.16.0
relations:
- delivers: epic:kubernetes-deployment-and-driver
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T11:43:06Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T11:43:06Z", actor: "human:timo", revision: 3}
---
## Outcome
Publish Substrate 0.16.0, the release of `wave/k8s-namespace-1`: the Kubernetes namespace driver of design 34 and ADR 0047 (`substrate-daemon --driver kubernetes`), the executor image `ghcr.io/beyond10x/b10x-substrate-executor:<version>`, the development bundle `substrate-wire/0.25.0`, the host execution AppArmor profile `substrate-host-exec-v2`, the local chart and namespace lanes with their release records, and the adversary fixes of this wave. Everything else under `[Unreleased]` since 0.15.0 ships with it (CHANGELOG.md `[0.16.0]`).

## Stated limits
The release notes say so: every exec start on the Kubernetes driver is refused `exec.sandbox-unavailable`, because the probe proves neither the four Linux confinement facts nor `confinement.no-egress`; exec on a cluster is unverified (design 34 § 10.3 O3); shared clean-room conformance follows in the next release (`story:remote-clean-room-conformance`, D186).

## Approval
Operator, 2026-10-29 ("looking forward to a release"; goal: 0.16.0 tagged and published with the Kubernetes namespace driver), under the coordinating session's standing release grant.

## Scope and verification
`Cargo.toml`, `Cargo.lock`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `CHANGELOG.md`, the generated release notes page, and the workspace package labels in `THIRD_PARTY_LICENSES.html`. Minor, not patch: a new driver, a new contract bundle, a one-way state store schema upgrade and a new default AppArmor profile. Verified by the pull request's full CI gate, the delegated lane, the project-quota lane, the chart lane and the namespace lane at the exact tagged commit, an annotated bare version tag with its `Confinement-lane:`, `Quota-lane:`, `Chart-lane:` and `Namespace-lane:` records, the release workflow, signatures and artifact read-back.
