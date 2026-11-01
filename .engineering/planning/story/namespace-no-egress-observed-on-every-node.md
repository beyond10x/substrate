---
format: aep.planning-md/3
id: story:namespace-no-egress-observed-on-every-node
kind: story
status: implemented
title: The namespace driver observes no-egress on every node an exec Pod can land on
owner: substrate
tags:
- confinement
- kubernetes
relations:
- depends_on: story:namespace-driver-admits-an-exec-on-observed-confinement
- informed_by: approval-record:d338-review-findings-decided-in-session
- informed_by: approval-record:d339-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: crates/substrate-kubernetes/src/exec.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/exec_tests.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/lib.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/probe.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/probe_tests.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/restart_tests.rs
revision: 17
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T15:22:59Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T15:13:33Z", actor: "human:timo", revision: 16}
- {from: "active", to: "implemented", at: "2026-11-01T20:46:30Z", actor: "human:timo", revision: 17, decided_on: {"recorded":{"test_result":3,"review_outcome":2,"verification":1}}}
---
# Story: the namespace driver observes no-egress on every node an exec Pod can land on

## Outcome

The namespace driver publishes `confinement.no-egress` only when its probe has observed egress
blocked on every schedulable node an exec Pod can be placed on, or it pins exec Pods to the nodes
it observed. A cluster that enforces NetworkPolicy on some nodes and not others no longer gets the
fact from a probe that happened to land on an enforcing node.

## Design

Filed from the wave 2026-10-31 security pass (`review-result:wave-2026-10-31-design-30-security-pass-1`,
judgement J3): the probe in `crates/substrate-kubernetes/src/namespace/probe.rs` runs one Pod on one
node and publishes the fact for exec Pods on every node. The reviewer could not show a cluster that
reaches this state. The choice between probing per node and pinning exec Pods to observed nodes
needs a design 34 amendment before implementation.

## Acceptance

Rewritten 2026-11-01 for the design 34 amendment of that date (D339): the guarantee is a refusal
on an unobserved node, not a withheld fact.

- An exec start whose workspace keeper is on a node other than the one the latest probe ran on is
  refused `exec.sandbox-unavailable` (501), recorded as refused, and no Pod is created.
- An exec start whose keeper is on the probe's node is dispatched as before.
- A probe whose keeper node could not be read proves no run fact.
- After a later probe lands on another node, the next start on a workspace on the first node is
  refused; an exec already running there is untouched.

## Adopted

Adopted 2026-11-01 under D338 (the 2026-10-31 findings triage sheet, row 1). Blocked until design
34 is amended to choose between probing every schedulable node and pinning exec Pods to the nodes
the probe observed; the amendment is the first unit of the wave that takes this story.

## Scope

Derived 2026-11-01 by `story-scoper` at `bf0e0443` (wave D354). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong). Design 34 is already amended (amendments of 2026-11-01, D339); this story's code does not edit it.

- **Probe side:** `crates/substrate-kubernetes/src/namespace/probe.rs` — `Probed` (:89), `RunProofs` (:100, derives `Copy`, so the node goes beside it in `Probed`), `capability_snapshot` (:411; digest material :473-480), `probe_run` (:609), `run_probe_pod` (:670; `scheduled_node(keeper)` at :679) — cited
- **Start side:** `crates/substrate-kubernetes/src/exec.rs` — `Execs::published` (:69), `publish_exec_snapshot` (:134), `prepare_start` (:217; `placement` :246, `check_binding` :248, `render_exec_pod` :256), `placement` (:295; keeper `spec.nodeName` read at :336-340) — cited. The comparison goes in `prepare_start` after `check_binding` and before the render, refused `DriverError::unserved("exec.sandbox-unavailable", …)` — inferred
- **Node binding (wave D354 decision):** the node is kept with the published snapshot tuple in the driver and is not part of the snapshot digest; design 34's amendment says the node is not a wire field and the refusal is the guarantee, so the client hears `exec.sandbox-unavailable` (501), not `exec.capability-stale` — decided by the coordinator
- **Also likely:** `crates/substrate-kubernetes/src/lib.rs` `machine()` (:94-98) only if the publish signature changes — inferred
- **Tests:** `crates/substrate-kubernetes/src/exec_tests.rs` (`rig()` :196, `NODE` :41, `POD_CREATE` :43, model `a_foreign_snapshot_or_a_moved_binding_is_stale_before_any_pod` :603); `crates/substrate-kubernetes/src/namespace/probe_tests.rs` (`NODE` :39, played kubelet hard-codes `NODE` at :252-253 and :821-825; `Play` :56-62); `crates/substrate-kubernetes/src/restart_tests.rs` (`take_over` :43-54) — cited for the helpers, inferred for the changes
- **Not touched:** `crates/substrate-kubernetes-client/src/fake.rs` (no node handling; tests set `spec.nodeName` through `admin.update`) — cited; `crates/substrate-wire` (`CapabilitySnapshot` unchanged) — cited; `crates/substrate-daemon/src/app/operations.rs` — cited
- **Documents:** `CHANGELOG.md` `[Unreleased]` — inferred
- **Confidence:** high for the files and symbols.
- **Would collide with:** any unit on `probe.rs`, `probe_tests.rs`, `exec.rs`, `exec_tests.rs`, `restart_tests.rs`; in wave D354 `story:driver-names-a-bare-exec-refusals-verb` keeps its tests out of `probe_tests.rs`.
