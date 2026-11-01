---
format: aep.planning-md/3
id: story:pod-sweep-replays-from-the-policys-create
kind: story
status: implemented
title: The sweep's replay starts from the policy's create
relations:
- decomposes: epic:confinement-contract-family
- informed_by: approval-record:d339-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/pod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod_cleanup_never_created.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod_cleanup_tests.rs
- confidence: cited
  path: docs/design/28-pod-confinement-adapter.md
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T02:22:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T15:13:33Z", actor: "human:timo", revision: 7}
- {from: "active", to: "implemented", at: "2026-11-01T20:46:30Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":3,"review_outcome":2}}}
---
# Story: the sweep's replay starts from the policy's create

## Observation

The D339 adversary pass on `story:pod-orphaned-policy-is-swept` (out of scope there): the
never-created rule replays the run's Pod from the policy's `resourceVersion`, which is the policy's
last write, not its create. The adapter never writes a policy after creating it, but another writer
(an operator adding a label or an annotation) after the Pod was created moves the replay's start
past the Pod's whole history. The replay then shows no event of the name, and the policy of a
force-deleted Pod whose container may still run is swept. Design 28 § *A gone Pod* states the
assumption that the adapter is the only writer of its policies.

## Acceptance

- A policy whose metadata shows a write after its create (another `managedFields` entry, or its
  one entry's time after `creationTimestamp`) is not swept by the never-created rule; it is kept
  and logged, as a policy whose container is not shown stopped is.
- A policy unchanged since its create is swept as before.
- A fake-API case holds both and runs once in seconds.

## Not known

Whether the sweep's metadata-only list (`PartialObjectMetadata`) carries `managedFields` on the
served Kubernetes versions, or the rule needs a whole read of each candidate.

## Scope

Derived 2026-11-01 by `story-scoper` (wave D354). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-host/src/confinement/pod.rs`, the `pod` adapter's open sweep (`sweep`, 437-512) and deferred sweep (`resweep`, 667-741) — cited
- **Files:**
  - `crates/substrate-host/src/confinement/pod.rs` — cited: root cause at 561, `PodTrace::since(policy["metadata"]["resourceVersion"])`. The write check goes in `sweep_page` (536-565), before `gone_pod` (518-523), which both sweeps call (475, 699).
  - `crates/substrate-host/src/confinement/pod_cleanup_never_created.rs` — inferred: the new case's home (`kept` 44, first test 57), registered at `pod_cleanup_tests.rs:901-902`.
  - `crates/substrate-host/src/confinement/pod_cleanup_tests.rs` — inferred: the test double `struct Cluster` (30), `list()` (204), `page()` (238, policy fixture with no `managedFields`).
  - `docs/design/28-pod-confinement-adapter.md` — cited: § *A gone Pod* (219-269); the "adapter is the only writer" sentence (265-269) is rewritten.
  - `CHANGELOG.md` — cited: `## [Unreleased]`.
- **Metadata list type:** `PartialObjectMetadataList`, parsed as raw `serde_json::Value` (pod.rs:550); `list_items` (`crates/substrate-kubernetes-client/src/api.rs:1039`) returns items unchanged, so `managedFields` survives the client parse — cited. Whether served Kubernetes versions put `managedFields` in a metadata-only list — unproven.
- **Not touched:** `crates/substrate-kubernetes-client/src/gone.rs`, `api.rs`, `lib.rs`, `fake.rs` — inferred (fake.rs: cited, no `managedFields`; sweep tests use the `pod_cleanup_tests.rs` double).
- **Confidence:** medium.
- **Would collide with:** `pod.rs` 437-741 sweeps; design 28 § *A gone Pod*; the `pod_cleanup_*` test double; `CHANGELOG.md` `[Unreleased]` (trivial).
