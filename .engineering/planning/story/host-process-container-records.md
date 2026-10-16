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
revision: 1
---
# Story: Docker container and k3d cluster records

## Context

Docker and k3d containers were 22 of the 57 leaks, left running or with restart policies. This is the minimum slice of the future workloads and endpoints families: records and lifecycle only.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S8). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-host/src/`: docker and k3d backends invoking the runtimes' own commands with restart policy `no` and the record, owner and renew-by labels; removal on stop and expiry; runtime observations.
- `crates/substrate-wire/src/lib.rs`: `backend`, the container and cluster requests, the `process.container.docker`/`process.container.k3d` facts and three refusals.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

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
