---
format: aep.planning-md/3
id: story:unconfirmed-pod-deletion-ends-unknown
kind: story
status: active
title: A deletion the kubelet does not confirm within 60 s ends as Unknown
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:exec-pod-past-wall-bound-is-deleted
scope:
- confidence: cited
  path: crates/substrate-kubernetes/src/exec.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/exec_tests.rs
- confidence: inferred
  path: docs/design/34-kubernetes-namespace-driver.md
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 6}
---
# Story: A deletion the kubelet does not confirm within 60 s ends as Unknown

## Outcome

When the Kubernetes namespace driver deletes an exec Pod (a kill signal, or the wall bound plus 60 s)
and the Pod is still present, with the uid the driver bound, 60 s after the delete was accepted, the
exec ends with an `Unknown` outcome. It never ends as `Cancelled`, which states the run is gone,
while the Pod may still be running.

## Finding

Fix unit R of `wave/k8s-namespace-1` (`fix/exec-wall-bound-pvc`, `f11222a8`) reported it, and the
coordinating session filed it on 2026-10-29:

- Design 28 says a Pod deleted on a kill "ends as a Kill", and that a kubelet that does not confirm
  the deletion within 60 s leaves the run `Unknown`.
- The kill path in `crates/substrate-kubernetes/src/exec.rs` ends the exec `Cancelled` once the
  delete is accepted. It does not wait for the Pod to be gone, and it has no `Unknown` arm.
- The wall-bound deletion added by fix R uses the same path, so it shares the gap.

## Acceptance

- A delete accepted for a Pod whose uid is still present 60 s later (on the driver's injected clock)
  ends the exec `Unknown`, and the kill intent stays recorded for the garbage round.
- A Pod gone within 60 s, or a name now holding another uid, ends the exec as today.
- A red case first, on the fake API: a Pod that ignores its delete, then a clock moved 60 s.
- Every new case runs once, in seconds; no real wait.

## Design

Design 28 (accepted) states the outcome. Design 34 § 7 (the exec lifecycle) gets a sentence if it
does not already say it. No contract change: `Unknown` is an existing outcome.

## Scheduling

Draft, for the first substrate wave after `wave/k8s-namespace-1`, beside
`story:remote-clean-room-conformance` and `story:kubernetes-collisions-metric` (the coordinating
session, 2026-10-29).
