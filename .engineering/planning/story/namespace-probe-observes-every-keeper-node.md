---
format: aep.planning-md/3
id: story:namespace-probe-observes-every-keeper-node
kind: story
status: proposed
title: The namespace probe observes every node a live workspace keeper runs on
relations:
- informed_by: story:namespace-no-egress-observed-on-every-node
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T00:35:28Z", actor: "human:timo", revision: 2}
---
# Story: the namespace probe observes every node a live workspace keeper runs on

## Outcome

On a cluster with several nodes, an exec start is admitted on every node that holds a workspace
keeper, not only on the node the latest probe happened to land on: the probe runs once per distinct
keeper node, and each node's observation is kept and expires on its own.

## Design

Follows the design 34 amendment of 2026-11-01 (D339), which admits an exec only on the probe's node.
The driver cannot list nodes (ADR 0047 decision 3), but it already lists its own keeper Pods and
reads their `spec.nodeName`; a probe keeper and Pod pinned to each such node (required
`kubernetes.io/hostname` affinity, as exec Pods have) observe it. Probe cost grows with the number
of keeper nodes; a bound on probes per round is part of the design.

## Acceptance

- Two workspaces whose keepers are on two nodes, both observed denying egress, each admit an exec.
- A node whose probe proved nothing refuses exec starts on it while the other node's still admit.
