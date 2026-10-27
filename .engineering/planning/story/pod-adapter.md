---
format: aep.planning-md/3
id: story:pod-adapter
kind: story
status: implemented
title: 'The pod adapter: one Kubernetes Pod per run'
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-contract-on-linux-and-local
- informed_by: story:kubernetes-namespace-driver-entry-gate
- depends_on: story:oci-adapter
scope:
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_pod.rs
- confidence: inferred
  path: docs/design/28-pod-confinement-adapter.md
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":5}}}
---
# The `pod` adapter: one Kubernetes Pod per run

ADR 0109 decision 3: `pod` is the outer adapter; `linux` may run inside it. The adapter renders a
profile into one Pod in a namespace the operator configured (never one the request names) and a
default-deny NetworkPolicy beside it, runs it through the cluster API, follows its log and phase,
and deletes both at the end. Pod posture: `runAsNonRoot`, a numeric `runAsUser`,
`readOnlyRootFilesystem`, `allowPrivilegeEscalation: false`, all capabilities dropped,
`seccompProfile: RuntimeDefault`, `automountServiceAccountToken: false`, resource requests equal to
limits from the profile's bounds, `activeDeadlineSeconds` from its wall bound, an image by digest,
workspaces as `emptyDir` volumes. It serves `process_run`, `process_watch`, `output_read`, `wait`,
`workspace`, `limits` and `audit`; `filesystem_view` of host paths and both network families are not
served (refused by name), because a NetworkPolicy's enforcement depends on the cluster's network
plugin and cannot be proved by the API. Inside `pod`, the host's policy fact `local_permitted` is
false (ADR 0109 F3 A).

The authority rules of `story:kubernetes-namespace-driver-entry-gate` that apply to one Pod are
taken here and written into `docs/design/28-pod-confinement-adapter.md` before code: a request
cannot choose the namespace, the ServiceAccount, a `hostPath`, `privileged`, a node name or a
runtime class; the adapter creates no cluster-scoped resource. The namespace driver of
`epic:kubernetes-deployment-and-driver` stays a separate, later piece.

## Acceptance

- Design 28 is in `docs/design/` and lists each refused Pod field with its refusal code.
- `crates/substrate-host/tests/ess_confinement_pod.rs` executes every `pod` scenario against a
  disposable cluster in CI (a pinned `kind` in a new `pod-lane` job of `.github/workflows/gate.yml`);
  with `SUBSTRATE_POD_LANE=1` an unreachable cluster fails the test; without it the lane is absent.
- The rendered Pod for a fixed profile equals a committed golden manifest byte for byte (portable
  test); every field the design refuses, set through the profile, is refused before any API call.
- A run that exceeds its memory bound ends with the container's `OOMKilled` reason, and its `wait`
  observation reports `memory_oom_kills` 1 (the counter the `linux` adapter reports,
  `crates/substrate-wire/src/lib.rs:2161`) and a non-zero exit; a run's Pod and NetworkPolicy are
  gone after `wait`, after a kill and after a deadline.
- An `aep:adversary` pass and an `aep:security-reviewer` pass (cluster credentials) are each
  recorded as a `review-result` that `reviews:` this story, and each of their findings has a
  `review_outcome` record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/src/confinement/pod.rs` (replaces the S0 stub),
`crates/substrate-host/tests/ess_confinement_pod.rs` and its golden manifest (new),
`docs/design/28-pod-confinement-adapter.md` (new), `.github/workflows/gate.yml` (a new `pod-lane`
job; `story:oci-adapter` adds an environment variable to the gate job of the same file, and this
story merges after it), `Cargo.toml`/`Cargo.lock` if a Kubernetes client crate is admitted (then
`cargo xtask check-licenses` and `check-advisories` apply), `CHANGELOG.md` (one line under
`## [Unreleased]`; the other units of its wave add theirs, and the merge into the integration
branch keeps every line).

## Depends on

`story:confinement-contract-on-linux-and-local`.
