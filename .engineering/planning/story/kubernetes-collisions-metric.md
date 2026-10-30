---
format: aep.planning-md/3
id: story:kubernetes-collisions-metric
kind: story
status: active
title: The operational listener exposes Kubernetes name collisions
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:operational-health-and-telemetry
- depends_on: story:kubernetes-namespace-workspace-and-exec-slice
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/health.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/ops_listener
- confidence: inferred
  path: crates/substrate-kubernetes/src/ownership.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 6}
---
# Story: The operational listener exposes Kubernetes name collisions

## Outcome

An operator of a `--driver kubernetes` daemon can read how often the driver met an object it did not
create under a name it would use (design 34 § 4), from the operational listener's metrics, without
reading logs.

## Acceptance

- `substrate_kubernetes_collisions_total` is served on the operational listener (ADR 0045 decision 7)
  by a daemon running the Kubernetes driver.
- Its name, labels and help text are pinned by a golden, beside the existing metrics goldens.
- A test increments it through a real collision on the fake API: an object of another deployment or
  a foreign uid under a name the driver would use, left untouched, and the scrape shows the count
  moved by exactly that collision.
- A host-driver daemon's scrape is unchanged.

## Design

ADR 0045 decision 7 governs the operational listener's series; design 34 § 4 defines a collision and
the counter the driver already keeps. If adding the series needs a change to what ADR 0045 decides,
that change comes before code (invariant 8).

## Scheduling

Decision D187, 2026-10-29, by the coordinating session: implemented in the first substrate wave after
`wave/k8s-namespace-1`, together with `story:remote-clean-room-conformance`. Found by U13b of design
34: the driver counts collisions, and nothing exposes the count.

## Boundaries

No new collision handling: the driver's behaviour on a collision (leave the object alone, count it)
does not change.
