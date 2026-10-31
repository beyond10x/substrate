---
format: aep.planning-md/3
id: story:namespace-driver-admits-an-exec-on-observed-confinement
kind: story
status: implemented
title: A namespace driver admits an exec on the confinement it observed
owner: substrate
tags:
- confinement
- kubernetes
relations:
- informed_by: story:confinement-adapter-on-the-wire
- depends_on: story:confinement-adapter-on-the-wire
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T11:27:59Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-31T11:28:48Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-31T16:30:11Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
# Story: a namespace driver admits an exec on the confinement it observed

## Outcome

A daemon running `--driver kubernetes` whose probe observed design 34 § 6.4's neutral set admits an
exec start instead of refusing it `exec.sandbox-unavailable`. Each of the six facts comes from an
observation in the namespace, never from configuration, and one that was not observed stays
absent, so the start is refused as today (invariant 3).

## Design

[Design 34](../../../docs/design/34-kubernetes-namespace-driver.md) § 6.2 (the probe's rows for the
families, whole-tree kill and no egress) and § 6.4 (admission), on the bundle `0.26.0` that
[ADR 0048](../../../adr/0048-the-wire-states-what-a-confinement-adapter-proves-never-its-name.md)
and `story:confinement-adapter-on-the-wire` cut. Filed under option A of
`decision-blocker:namespace-exec-needs-design-30`, its § *What follows A*, item 3.

## Acceptance

- The probe Pod's end, its decoded log frames and an `applied` record that matches the rendered
  memory and CPU publish `confinement.family.process-run`, `.process-watch`, `.output-read`,
  `.wait`, `.limits`, `.audit` and `.workspace`; any one of them failing leaves all seven absent.
- `probe-child` forks a grandchild that writes a heartbeat to the PVC every 100 ms and does not
  exit; after the container shows `terminated`, the keeper's `fs stat` sees the heartbeat unchanged
  for 2 s, which publishes `confinement.whole-tree-kill`. A heartbeat that moves leaves it absent.
- From inside the probe Pod, TCP to `KUBERNETES_SERVICE_HOST:443`, a DNS query to
  `/etc/resolv.conf`'s nameserver and TCP to the configured `egress_probe_address` each fail within
  3 s, which publishes `confinement.no-egress`. Any connection that succeeds, and any observation
  that cannot be read, leaves it absent.
- `confinement_proved` in `crates/substrate-daemon/src/app/operations.rs` admits on the four Linux
  facts or on the six neutral facts. Fake-API cases in `crates/substrate-kubernetes` and
  `crates/substrate-daemon/tests/namespace_admission.rs` show: all six observed, an exec start is
  dispatched as a Pod; any one of the six missing, `exec.sandbox-unavailable` (501) with no Pod.
  The host driver's answers do not change.
- The four Linux facts are never published by this driver.

## Boundaries

- The kind lane's network plugin and its exec cases are
  `story:namespace-lane-runs-exec-on-an-enforcing-network-plugin`.
- No wire change beyond bundle `0.26.0`.
