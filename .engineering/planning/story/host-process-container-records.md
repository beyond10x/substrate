---
format: aep.planning-md/3
id: story:host-process-container-records
kind: story
status: draft
title: Docker container and k3d cluster records
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-daemon/src/app/processes.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/processes.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/host_process
- confidence: inferred
  path: crates/substrate-store/src/host_processes.rs
- confidence: inferred
  path: crates/substrate-wire/src/generated/host_process.rs
- confidence: cited
  path: crates/substrate-wire/src/host_process_tests.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: cited
  path: spec/host-processes
- confidence: cited
  path: spec/host-processes-suite.json
- confidence: inferred
  path: xtask/bundle-source/0.23.0
- confidence: inferred
  path: xtask/src/bundle/host_process.rs
revision: 4
---
# Story: Docker container and k3d cluster records

## Context

Docker and k3d containers were 22 of the 57 leaks, left running or with restart policies. This is the minimum slice of the future workloads and endpoints families: records and lifecycle only.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S8). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-host/src/host_process/` (docker and k3d backends) — cited
- **Files:** `spec/host-processes/` (`processes.yaml:705` `StartContainer`, deferred points 11 and 12; `wire.yaml`; `scenarios/`; `README.md:49`), `spec/host-processes-suite.json`, `crates/substrate-wire/src/lib.rs` (`PROCESS_REFUSALS` :2555, facts :3369, `ADVERTISED_CONTRACT_BUNDLE` :18), `crates/substrate-wire/src/host_process_tests.rs:310` — cited
- **Also likely:** `crates/substrate-wire/src/generated/host_process.rs`, successor bundle `0.23.0`, `scripts/gate.sh`, `xtask/src/bundle/host_process.rs`, `crates/substrate-daemon/src/app/processes.rs`, `crates/substrate-store/src/host_processes.rs`, `runtime_vectors.rs`, `crates/substrate-daemon/tests/processes.rs`, `CHANGELOG.md`, `AGENTS.md` — inferred
- **Confidence:** medium — ADR 0036 S8 says `backend`, the spec draft says `kind` plus raw `argv`
- **Open before code:** points 11 (label keys) and 12 (container timeout) and the request shape, settled spec-first; whether the restart and label refusals hold against raw runtime argv is not shown.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-container-runs-with-restart-no-and-owner-lease-labels`
- `host-container-lease-expiry-removes-the-container`
- `host-cluster-lease-expiry-deletes-the-cluster`
- `host-container-is-listed-with-runtime-observations`
- `host-container-record-reports-confinement-none`
- `host-container-caller-restart-policy-is-refused` (`process.container-restart-refused`)
- `host-container-reserved-label-is-refused` (`process.container-label-reserved`)
- `host-container-unserved-without-a-runtime` (`process.container-unserved`)
