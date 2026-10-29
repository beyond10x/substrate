---
format: aep.planning-md/3
id: story:kubernetes-namespace-workspace-and-exec-slice
kind: story
status: active
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
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:54:34Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-10-29T02:08:26Z", actor: "human:timo", revision: 3}
---
# Story: Kubernetes namespaces serve workspace and exec

## Outcome

A configured namespace can back Substrate workspaces with PVCs and bounded argv-only executions with one-shot Rust executor pods while clients continue to use the same public contract.

## Acceptance

Workspace creation durably records intent before PVC creation and records PVC UID as backend identity. File operations use a Rust executor path with beneath/no-link/no-mount semantics or name an unserved guarantee; no shell command performs file I/O. Exec start durably records intent before Pod creation, uses an immutable image digest, argv, cleared/shaped environment, non-root security context, read-only root filesystem, explicit workspace mount, resource requests/limits, deadline and default-deny network. Output is drained and bounded, cancellation deletes the whole execution object set, and restart reconciliation handles watch gaps by UID and resourceVersion. Capability facts describe verified properties, never the Kubernetes driver name. Shared conformance proves idempotency, lost answers, quota/capacity refusal, cleanup and absent unsupported capabilities.

## Out of Scope

Deployments, Services, ingress, image builds, arbitrary manifests, hostPath, privileged pods and cluster scheduling policy.

## Lane observations (design 34 § 10.3, U5)

Recorded run on `kindest/node:v1.37.0` (`xtask/src/namespace_lane.rs:51`):
`Namespace-lane: 425ab9c2709b54bc46c234b4646913d8f12f27a2 8 2026-10-29T05:19:33Z`, merged into
`wave/k8s-namespace-1` as `d1e51243`.

| Observation | Result | Consequence for U8 to U13 |
|---|---|---|
| O1 claim deletion waits for the Pod | did not hold: 60 s after background deletion the Pod still runs and the claim waits on `pvc-protection` | the driver's Pods-first order is the only cleanup; the backstop is dropped from the claim (§ 10.3) |
| O2 restricted namespace refuses a non-conforming Pod | held: 403 `violates PodSecurity "restricted:latest"`; unlabelled namespace 201 and nothing stored | as designed |
| O3 default-deny stops egress | did not hold: the API connection succeeds, DNS and outside time out | `confinement.no-egress` stays absent and exec cases are absent until the lane installs a pinned enforcing plugin (§ 10.3) |
| O4 keeper and a second Pod share one claim on one node | held | as designed |
| O6 cgroup bounds | held: `pids.max=76860`, `memory.max=33554432`, `cpu.max=10000 100000` | `pids.max` does not show `podPidsLimit` 1024: taken on the operator's word (§ 14 Q2), O11 proves it |
| O7 quota refusal message | did not hold: the message starts `persistentvolumeclaims "…" is forbidden: exceeded quota: …` | conflicts with § 10.2 step 6; sent to the orchestrator |
| O8 rules review | held after the pinned default set gained `clustertrustbundles` get/list/watch and four issuer-discovery URLs | as designed |
| O10 exec through `pods/exec` v5 | did not hold for the driver: 403 `cannot get resource "pods/exec"`; with admin credentials 200000 bytes and `Success` | v5 works; ADR 0047 decision 3 grants only `create`; sent to the orchestrator |
