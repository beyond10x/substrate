---
format: aep.planning-md/3
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
scope:
- confidence: inferred
  path: .github/workflows/release.yml
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Dockerfile
- confidence: cited
  path: ROADMAP.md
- confidence: cited
  path: STATUS.md
- confidence: cited
  path: crates/substrate-daemon/src/main.rs
- confidence: inferred
  path: deploy/helm/substrate
- confidence: inferred
  path: docs/design/04-security-and-isolation.md
- confidence: inferred
  path: xtask/src
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:54:32Z", actor: "human:timo", revision: 2, imported: true}
---
# Story: Kubernetes serves one stable address per node-bound daemon

## Outcome

Operators can install a public, signed Helm chart that gives each host-backed Substrate daemon a stable network identity, durable state and an explicit selected-node relationship.

## Acceptance

The chart deploys a StatefulSet on a dedicated labelled and tainted node pool with pod anti-affinity, one owner-private PVC per ordinal, a headless Service and stable per-pod DNS. Callers address a specific pod identity; no generic Service round-robins mutation requests across independent operation ledgers. TLS identity and hosted trust are mandatory outside loopback. Values expose explicit driver prerequisites and refuse unsupported privilege, cgroup, mount or quota postures rather than claiming absent facts. RBAC is minimal, default NetworkPolicy denies unrelated ingress/egress, Pod Security settings are documented, and upgrades preserve identity and state. The chart is packaged, provenance-attested, anonymously fetchable and tested by install/upgrade/uninstall conformance.

## Boundaries

This profile runs the host driver inside Kubernetes. It is not the namespace-scoped Kubernetes driver, a fleet scheduler or high availability for one ledger.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** a new Helm chart, `deploy/helm/substrate/` — inferred (`deploy/` holds only `apparmor/` and `seccomp/`)
- **Files:** `.github/workflows/release.yml` (chart packaging and signing), an `xtask/src` conformance verb, `Dockerfile` — inferred
- **Files:** `crates/substrate-daemon/src/main.rs` (`--allow-uid` :241; read only) — cited
- **Documents:** `ROADMAP.md`, `STATUS.md` — cited; a new design document or ADR, `docs/design/04-security-and-isolation.md`, `CHANGELOG.md`, `AGENTS.md` § Releases — inferred
- **Confidence:** medium
- **Blockers:** no design document or ADR for the serving profile (invariant 8); install/upgrade/uninstall conformance needs a Kubernetes cluster with a labelled, tainted node pool; chart distribution and signing is an operator decision.
