---
format: aep.planning-md/3
id: story:host-process-adopt
kind: story
status: draft
title: Adopt an existing pid, unit or container as an ownerless record
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
- depends_on: story:host-process-cgroup-tree
scope:
- confidence: inferred
  path: crates/b10x-substrate-sdk/src/model.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/processes.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/routes.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/host_process
- confidence: inferred
  path: crates/substrate-store/src/host_processes.rs
- confidence: inferred
  path: crates/substrate-wire/src/generated
- confidence: cited
  path: crates/substrate-wire/src/host_process_tests.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: cited
  path: spec/host-processes
- confidence: inferred
  path: xtask/bundle-source/0.23.0
revision: 4
---
# Story: adopt an existing pid, unit or container as an ownerless record

## Context

The survivors already running need a one-time pickup into records that can be listed, leased and stopped.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S9). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-daemon/src/app/processes.rs` (`POST /v1/processes/adoptions`) — cited
- **Files:** `crates/substrate-daemon/src/app/routes.rs:112-120`, `crates/substrate-host/src/host_process/`, `spec/host-processes/` (`processes.yaml:657` `AdoptProcess`, deferred point 10), `crates/substrate-wire/src/lib.rs:2556` (`PROCESS_REFUSALS`), `crates/substrate-wire/src/host_process_tests.rs:310-321` — cited
- **Also likely:** successor bundle `0.23.0` (the story's "after 0.18.0" is stale; `0.22.0` is current), `crates/substrate-store/src/host_processes.rs:240` (`owner_session`, `argv`, `cwd`, `lease` are not optional today), `runtime_vectors.rs`, `scripts/gate.sh`, `crates/substrate-wire/src/generated/`, `crates/b10x-substrate-sdk/src/model.rs` — inferred
- **Confidence:** medium
- **Blockers:** deferred point 10 (adopted supervision) and point 3 (lease, profile) are undecided (`approval-record:wave-2026-10-17-adopt-hostproc`); ADR 0036 (`:209`, "a lease the adoption sets") and the spec draft (`processes.yaml:423,661`, "no owner, no lease") disagree; unit and container adoption need a systemd user manager and Docker.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-adopt-pid-moves-its-whole-cgroup`
- `host-adopt-unit-stops-through-its-manager`
- `host-adopt-container-sets-restart-no`
- `host-adopted-record-is-ownerless-and-marked-adopted`
- `host-adopted-record-claims-no-unobserved-argv-log-or-exit`
- `host-adopt-absent-target-is-refused` (`process.adopt-target-absent`)
- `host-adopt-foreign-target-is-refused` (`process.adopt-target-foreign`)
- `host-adopt-unmovable-pid-is-refused` (`process.adopt-unmovable`)
