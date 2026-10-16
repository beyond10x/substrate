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
revision: 1
---
# Story: adopt an existing pid, unit or container as an ownerless record

## Context

The survivors already running need a one-time pickup into records that can be listed, leased and stopped.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S9). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-daemon/src/app/`: `POST /v1/processes/adoptions`.
- `crates/substrate-host/src/`: moving a pid's cgroup membership under the delegated root; adopting a user unit by its control group; adopting a container and setting restart policy `no`.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

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
