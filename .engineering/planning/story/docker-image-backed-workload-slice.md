---
format: aep.planning-md/1
id: story:docker-image-backed-workload-slice
kind: story
status: proposed
title: Docker serves immutable image-backed workloads
summary: Digest-resolved images and a minimal durable workload lifecycle extend the Docker driver without mutable-tag or caller-option authority.
owner: substrate
tags:
- docker
- images
- workloads
relations:
- decomposes: epic:container-driver-entry
- depends_on: story:docker-workspace-and-exec-slice
revision: 2
---
# Story: Docker serves immutable image-backed workloads

## Outcome

A caller can create, start, observe, stop and retire a minimal image-backed workload through Substrate while every runtime input is resolved to immutable evidence.

## Design gate

Before code, an accepted design fixes the workload and image resource state machines, digest resolution, pull authority, restart policy, lease behavior, event vocabulary, output bounds, reconciliation and refusal order. A successor bundle adds only those closed operations.

## Acceptance

An operator allowlists registries and credentials outside caller payloads. A caller may name an allowed image reference, but the driver resolves it once to a digest and records that digest before workload dispatch. Mutable tags are never retained as execution identity. The closed container spec inherits the Docker exec slice authority restrictions. Workload operations are durable before Docker mutation, idempotent by operation ID, reconcile by immutable container identity after restart, and terminalize with exact observed state. Pull, digest, platform, capacity, policy and capability failures are distinct named refusals. Shared black-box conformance covers lost answers, restart, lease expiry, cleanup and unauthorized runtime options.

## Out of Scope

Image builds, Compose semantics, arbitrary Docker APIs, fleet scheduling and Kubernetes workloads.
