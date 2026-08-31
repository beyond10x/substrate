---
format: aep.planning-md/1
id: story:kubernetes-namespace-workspace-and-exec-slice
kind: story
status: proposed
title: Kubernetes namespaces serve workspace and exec
summary: PVC-backed workspaces and one-shot executor pods implement the shared contract with durable Kubernetes dispatch and exact observations.
owner: substrate
tags:
- driver
- exec
- kubernetes
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:kubernetes-namespace-driver-entry-gate
revision: 2
---
# Story: Kubernetes namespaces serve workspace and exec

## Outcome

A configured namespace can back Substrate workspaces with PVCs and bounded argv-only executions with one-shot Rust executor pods while clients continue to use the same public contract.

## Acceptance

Workspace creation durably records intent before PVC creation and records PVC UID as backend identity. File operations use a Rust executor path with beneath/no-link/no-mount semantics or name an unserved guarantee; no shell command performs file I/O. Exec start durably records intent before Pod creation, uses an immutable image digest, argv, cleared/shaped environment, non-root security context, read-only root filesystem, explicit workspace mount, resource requests/limits, deadline and default-deny network. Output is drained and bounded, cancellation deletes the whole execution object set, and restart reconciliation handles watch gaps by UID and resourceVersion. Capability facts describe verified properties, never the Kubernetes driver name. Shared conformance proves idempotency, lost answers, quota/capacity refusal, cleanup and absent unsupported capabilities.

## Out of Scope

Deployments, Services, ingress, image builds, arbitrary manifests, hostPath, privileged pods and cluster scheduling policy.
