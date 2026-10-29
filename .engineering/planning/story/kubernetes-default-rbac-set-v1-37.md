---
format: aep.planning-md/3
id: story:kubernetes-default-rbac-set-v1-37
kind: story
status: implemented
title: The driver's pinned default RBAC set matches a v1.37 cluster
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:kubernetes-namespace-workspace-and-exec-slice
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T08:50:31Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T08:50:32Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-29T12:18:58Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: The driver's pinned default RBAC set matches a v1.37 cluster

## Outcome

A `--driver kubernetes` daemon whose ServiceAccount holds exactly ADR 0047 decision 3's Role plus the
rules every authenticated subject holds on a conformant v1.37 cluster opens, instead of refusing
`kubernetes.rbac-excess`.

## Finding

Adversary pass of 2026-10-29 on `wave/k8s-namespace-1` at `661d68e0`, scope namespace, red case
`driver_tests::the_default_grants_a_v1_37_cluster_shows_open_without_excess` on `adv/k8s-namespace`
(`1d607e8a`), verified by the coordinator. U13b found the same gap independently.

- `DEFAULT_RESOURCE_RULES` and `DEFAULT_NON_RESOURCE_RULES` in
  `crates/substrate-kubernetes/src/rules.rs` lack `get`, `list` and `watch` on
  `certificates.k8s.io` `clustertrustbundles` and the four issuer-discovery URLs that the first lane
  run observed (design 34 § 10.3 O8).
- Only the lane's copy in `xtask/src/namespace_lane_observe.rs` carries them.

## Acceptance

- `rules.rs` holds the observed set as the single source; the lane's copy uses it.
- The red case above passes; a review carrying one more rule is still `kubernetes.rbac-excess`.

## Design

Design 34 § 10.3 O8: "the pinned default set is corrected from the observation". No contract change.

## Scheduling

Implemented within `wave/k8s-namespace-1` by unit U13a, which needs it for lane step 3.
