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
- depends_on: story:operational-health-and-telemetry
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
revision: 6
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

## Scope

Derived 2026-10-27 by `story-scoper` and updated at 4decf8d1 for the proposed ADR. Every line is
**cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** a new chart under `deploy/helm/substrate/`, named `b10x-substrate` (Chart.yaml, values.yaml, a closed values.schema.json; templates for the StatefulSet with one PVC per ordinal, the headless Service, ServiceAccount without a token or Role, the default-deny NetworkPolicy, the init container that copies TLS and Identity CA material into a memory `emptyDir`, and the optional node-profile DaemonSet in its own namespace), as proposed by ADR 0046 (`adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md`, status `proposed`) — inferred; `deploy/` holds only `apparmor/` and `seccomp/`
- **Files:** a new `.github/workflows/chart-lane.yml` (ADR 0046 K9 B: static chart checks first, then a three-node kind cluster reusing `gate.yml`'s pinned kind v0.33.0 and `kindest/node:v1.37.0` at :175-177); `.github/workflows/gate.yml` itself unchanged — inferred
- **Files:** the lane runner as a new `cargo xtask` verb in `xtask/src/` plus `xtask/src/main.rs` (language rule) — inferred
- **Files:** `.github/workflows/release.yml` — inferred; package the OCI chart after the daemon image push so its default values pin the image digest, keyless-sign, verify, attest provenance with `actions/attest-build-provenance` (`attestations: write` on the release job only), anonymously read back, and refuse a tag without a successful `chart-lane.yml` conclusion; next to the bundle steps (:630-790). Provenance is disabled today for images: "SLSA provenance is a later milestone" (:74-76, cited); ADR 0046 K7 C leaves the image setting as it is
- **Files:** `xtask/tests/release_workflow.rs` — inferred; the new preflight refusal and the fourth artifact
- **Files (only if the operator picks ADR 0046 K3 B or K2 C):** `crates/substrate-daemon/src/tls.rs` `read_bounded_regular_file` (:108-126) and `crates/substrate-daemon/src/hosted.rs` `load_connector` (:373-385) — inferred that they change; their `O_NOFOLLOW` and owner rules are cited. Under the recommended K3 A no daemon code changes
- **Read only:** `crates/substrate-daemon/src/main.rs` flags (`tls_listen` :407 with `requires_all` hosted identity :395-445, `conflicts_with = "tcp_listen"` :405); `runtime.rs:573-578` (TLS and hosted Identity require each other; with TLS, `--allow-uid` is not required); `runtime.rs` `prepare_private_state_path` (:1632-1659: the state database's parent directory must be uid-owned and 0700, so `--state` names a subdirectory of the PVC); `Dockerfile` `daemon`/`release` targets (:61-74, :98) already ship `substrate-container-exec` and `substrate-container-profiles` — cited
- **Symbols:** `read_bounded_regular_file`, `load_connector`, `prepare_private_state_path`, `hosted::AUDIENCE` (`urn:b10x:substrate`, hosted.rs:37), `read_bearer_digest`'s `root_projected_group` (runtime.rs:1582-1585) as the precedent ADR 0046 K3 B would extend — cited
- **Documents:** `adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md` and its `adr/README.md` row (drafted, `proposed`; `cargo xtask check-adrs` refuses `proposed` until acceptance); `ROADMAP.md:28`, `STATUS.md:33-35` — cited; `website/docs/guides/deployment.md` (per-pod addressing, rotation by rolling restart under K3 A, Pod Security namespace labels, NetworkPolicy CIDR values), `README.md` § Container execution, `AGENTS.md` § Releases (fourth artifact, chart-lane preflight) and § The gate, `CHANGELOG.md` [Unreleased] — inferred
- **Depends on:** `story:operational-health-and-telemetry` (ADR 0045) for the probe paths and port (ADR 0046 K8 A); the store has no `depends_on` edge for it yet — cited from ADR 0046, edge not recorded
- **Confidence:** medium — the chart and CI surfaces are clear; the exact file set waits on the operator's answers to ADR 0046 K1-K9, and on ADR 0045
- **Would collide with:** any unit touching the daemon's TLS or Identity trust-root file loading (`tls.rs`, `hosted.rs`) if K3 B is chosen; the release job in `release.yml`; the `adr/README.md` index; `website/docs/guides/deployment.md`; `AGENTS.md` § Releases; `story:operational-health-and-telemetry` on `adr/README.md`, `STATUS.md`, `CHANGELOG.md` and the deployment guide
- **Safety fact:** mutation de-duplication is per ledger: one SQLite file per pod, keyed `(deployment, subject, operation)` (`xtask/src/render.rs:740`, ADR 0005). A retry that reaches another ordinal runs again. A headless Service's own DNS name resolves to every ready pod, so the chart is safe only if certificate SANs name per-pod hosts and never the Service name; then a client addressing the Service fails TLS verification instead of being spread across pods (ADR 0046 K2) — unproven until the chart lane asserts it
- **Blocker:** ADR 0046 is `proposed`; invariant 8 holds code until the operator decides K1-K9 and the ADR is accepted, and ADR 0045 is accepted first (K8 A)
