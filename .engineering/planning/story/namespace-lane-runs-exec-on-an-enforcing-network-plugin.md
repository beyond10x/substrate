---
format: aep.planning-md/3
id: story:namespace-lane-runs-exec-on-an-enforcing-network-plugin
kind: story
status: proposed
title: The namespace lane runs its exec cases on a network plugin that enforces NetworkPolicy
owner: substrate
tags:
- kubernetes
- lane
relations:
- depends_on: story:namespace-driver-admits-an-exec-on-observed-confinement
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T11:27:59Z", actor: "human:timo", revision: 2}
---
# Story: the namespace lane runs its exec cases on a network plugin that enforces NetworkPolicy

## Outcome

`cargo xtask namespace-lane observe` creates its kind cluster with a pinned network plugin that
enforces NetworkPolicy, so the driver's probe observes `confinement.no-egress` there, and the lane's
exec cases run instead of being reported absent.

## Design

[Design 34](../../../docs/design/34-kubernetes-namespace-driver.md) § 10.2 and § 10.3 O3: kindnet
does not enforce NetworkPolicy, so O3 did not hold and no exec could be admitted on the lane.
Filed under option A of `decision-blocker:namespace-exec-needs-design-30`, § *What follows A*,
item 3 ("the lane installing a pinned enforcing CNI").

## Acceptance

- The lane's kind cluster disables the default plugin and installs one plugin that enforces
  NetworkPolicy, pinned by version and image digest; the lane record names it.
- O3 holds on that cluster: the probe connections fail under the default-deny policy and the
  control Pod makes all three.
- The machine read in step 3 states `confinement.no-egress` and the seven families of design 34
  § 6.2; an exec start is admitted.
- The four exec cases now in `ABSENT` (`S6 quota exec.capacity`, `S6 restart: running exec reads
  unknown and its Pod is gone`, `S6 force-deleted Pod is not reported ended early`, `S6 Role
  without pods watch`) are executed and pass; the remaining absent cases keep their reasons.
- A recorded lane run for the tagged commit of the release that ships it.

## Boundaries

- Which plugin a deployment runs stays the operator's; the lane chooses one only so the observation
  can hold.
- Depends on `story:namespace-driver-admits-an-exec-on-observed-confinement`.
