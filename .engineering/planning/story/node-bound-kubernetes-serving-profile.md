---
format: aep.planning-md/1
id: story:node-bound-kubernetes-serving-profile
kind: story
status: proposed
title: Kubernetes serves one stable address per node-bound daemon
summary: A signed public Helm chart deploys stateful host-backed daemons without round-robin mutation routing.
owner: substrate
tags:
- helm
- kubernetes
- remote
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:production-tls-control-listener
- depends_on: story:hosted-trust-envelope-admission
- depends_on: story:network-session-authority
revision: 2
---
# Story: Kubernetes serves one stable address per node-bound daemon

## Outcome

Operators can install a public, signed Helm chart that gives each host-backed Substrate daemon a stable network identity, durable state and an explicit selected-node relationship.

## Acceptance

The chart deploys a StatefulSet on a dedicated labelled and tainted node pool with pod anti-affinity, one owner-private PVC per ordinal, a headless Service and stable per-pod DNS. Callers address a specific pod identity; no generic Service round-robins mutation requests across independent operation ledgers. TLS identity and hosted trust are mandatory outside loopback. Values expose explicit driver prerequisites and refuse unsupported privilege, cgroup, mount or quota postures rather than claiming absent facts. RBAC is minimal, default NetworkPolicy denies unrelated ingress/egress, Pod Security settings are documented, and upgrades preserve identity and state. The chart is packaged, provenance-attested, anonymously fetchable and tested by install/upgrade/uninstall conformance.

## Boundaries

This profile runs the host driver inside Kubernetes. It is not the namespace-scoped Kubernetes driver, a fleet scheduler or high availability for one ledger.
