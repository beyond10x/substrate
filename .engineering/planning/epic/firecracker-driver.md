---
format: aep.planning-md/1
id: epic:firecracker-driver
kind: epic
status: proposed
title: Direct Firecracker driver
summary: A KVM- and jailer-gated direct Firecracker backend runs immutable boot artifacts and confined workspace/exec slices with truthful capability facts.
owner: substrate
tags:
- driver
- firecracker
- microvm
relations:
- depends_on: epic:remote-serving
revision: 2
---
# Epic: Direct Firecracker driver

## Outcome

A Substrate driver can execute one bounded job in a fresh Firecracker microVM while preserving durable operation semantics, immutable input provenance, least-authority mounts, resource bounds and exact observations.

## Boundaries

The driver talks directly to Firecracker and its jailer; it does not shell out to a general VM manager or ask Kubernetes to emulate the driver contract. Images and kernel/rootfs artifacts are operator-selected immutable deployment inputs, never caller-controlled host paths. Missing /dev/kvm, jailer prerequisites or artifact verification are named unserved refusals.

## Environment constraint

The currently observed development EKS nodes are T3a/T4g instances with no RuntimeClass, KVM device or microVM device plugin. Live conformance therefore requires a dedicated tainted KVM-capable node pool; absence on the present nodes is expected evidence, not a pass.

## Delivery

An entry and immutable-boot-artifact gate precedes a minimal workspace/exec slice. PTY, egress, secret slots and multi-VM workload orchestration are later stories.
