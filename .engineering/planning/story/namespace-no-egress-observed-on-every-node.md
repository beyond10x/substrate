---
format: aep.planning-md/3
id: story:namespace-no-egress-observed-on-every-node
kind: story
status: proposed
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
- confidence: cited
  path: docs/design/34-kubernetes-namespace-driver.md
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T15:22:59Z", actor: "human:timo", revision: 2}
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
