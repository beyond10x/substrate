---
format: aep.planning-md/3
id: story:remote-clean-room-conformance
kind: story
status: implemented
title: A clean-room client proves remote Substrate
summary: A black-box Rust runner verifies HTTPS/WSS, auth, durable recovery and event semantics against any deployed instance.
owner: substrate
tags:
- conformance
- remote
- testing
relations:
- decomposes: epic:remote-serving
- depends_on: story:remote-sdk-transport
- depends_on: story:node-bound-kubernetes-serving-profile
- depends_on: story:operational-health-and-telemetry
scope:
- confidence: cited
  path: .github/workflows/chart-lane.yml
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: crates/b10x-substrate-conformance
- confidence: cited
  path: scripts/delegated-lane.sh
- confidence: inferred
  path: website/docs/guides
- confidence: cited
  path: website/sidebars.ts
- confidence: cited
  path: xtask/src/chart_cluster.rs
- confidence: cited
  path: xtask/src/namespace_lane_run.rs
- confidence: cited
  path: xtask/src/packages.rs
revision: 17
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:54:33Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 16}
- {from: "active", to: "implemented", at: "2026-10-30T20:34:26Z", actor: "human:timo", revision: 17, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
# Story: A clean-room client proves remote Substrate

## Outcome

A deployed Substrate endpoint can be certified from outside its process, container and repository checkout using only its public contract and a scoped test authority.

## Acceptance

A Rust conformance runner accepts an HTTPS endpoint, trust roots and a token-provider command or file descriptor without persisting credentials. It verifies machine facts, workspace/file/exec lifecycle, operation-id recovery after a deliberately lost response, restart-to-unknown behavior, event ordering and retention gaps, TLS/auth negatives, raw-pipe or PTY WSS attachment when advertised, exact resource observations and cleanup. Capability-dependent cases are reported passed, failed or absent; absent is never counted as passed. The runner produces a machine-readable report and its SHA-256 digest file, carrying endpoint identity, advertised contract, capability snapshot digest and case inventory but no credential, argv output or tenant data; the certification run signs the report with a detached signature (design 31 § 6), and the runner holds no signing key.

The same wave turns on step 4 of the Kubernetes namespace lane (design 34 § 10.2): the lane inventory change that makes `cargo xtask namespace-lane observe` run this runner against the `--driver kubernetes` daemon, with `--restart-hook`, is part of this story's acceptance.

## Design

[Design 31](../../../docs/design/31-remote-clean-room-conformance-runner.md) (proposed) is the design this story is built from (invariant 8).

## Scheduling

Decision D186, 2026-10-29, accepted by the coordinating session on one condition: this story is
implemented in the first substrate wave after `wave/k8s-namespace-1`. Wave `k8s-namespace-1` ships
design 34 § 10.2 step 4 as absent, named in the lane inventory, because this runner does not exist
yet. The step is postponed, not dropped: the operator accepted design 34 as the right choice rather
than the cheapest, and step 4 is part of it.

## Boundaries

The runner is a client and links no daemon, host driver or store implementation. It does not provision clusters or choose a production fleet.
