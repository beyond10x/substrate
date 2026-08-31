---
format: aep.planning-md/1
id: story:kubernetes-namespace-driver-entry-gate
kind: story
status: proposed
title: The Kubernetes namespace driver has a closed authority gate
summary: RBAC, namespace ownership, resource identity, reconciliation and cleanup are fixed and probed before Kubernetes-backed execution code.
owner: substrate
tags:
- driver
- kubernetes
- security
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:node-bound-kubernetes-serving-profile
- depends_on: story:remote-clean-room-conformance
- depends_on: story:driver-port-carries-no-host-types
revision: 2
---
# Story: The Kubernetes namespace driver has a closed authority gate

## Outcome

A namespace-scoped Kubernetes driver can be implemented against an accepted, mechanically checked authority and lifecycle design rather than discovering its contract through cluster side effects.

## Acceptance

An accepted design or ADR fixes configured namespace ownership, ServiceAccount and minimal RBAC, object naming and labels, server-side-apply ownership, finalizers, durable-before-API dispatch, immutable backend identity, watch gaps, restart reconciliation, cancellation, garbage collection, quotas, NetworkPolicy defaults and refusal order. A startup probe publishes only facts it proves with disposable resources and cleans them. Requests cannot select namespaces, service accounts, host paths, privileged settings, node names, runtime classes or arbitrary pod fields. The development cluster conformance creates no cluster-scoped resource and leaves no object after cleanup.

## Environment boundary

The currently observed dev cluster has healthy general-purpose nodes but no RuntimeClass or KVM device. That is sufficient for the namespace driver entry gate and ordinary pod execution; it makes no microVM claim.
