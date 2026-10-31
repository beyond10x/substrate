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
revision: 2
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

- A probe test with two nodes, one of which reports egress reachable, withholds
  `confinement.no-egress`.
- A probe test where every node reports blocked publishes it.
