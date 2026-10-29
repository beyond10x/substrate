---
format: aep.planning-md/3
id: story:exec-pod-past-wall-bound-is-deleted
kind: story
status: implemented
title: An exec Pod past its wall bound is deleted
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:kubernetes-namespace-workspace-and-exec-slice
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T08:55:28Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T08:55:28Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-29T12:18:56Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: An exec Pod past its wall bound is deleted

## Outcome

An exec whose Pod has not ended 60 s after its wall bound (`activeDeadlineSeconds`) is ended by the
driver: its Pod is deleted under the uid rule and the exec leaves `running`, as design 34 § 9 and
design 28 state. A Pod the kubelet never started cannot hold an exec `running` forever.

## Finding

Adversary pass of 2026-10-29 on `wave/k8s-namespace-1` at `661d68e0`, scope exec, red case
`exec_tests::a_pod_never_started_is_deleted_sixty_seconds_past_its_wall_bound` on `adv/k8s-exec`
(`41e746e7`), verified by the coordinator:

- Design 34 § 9: "The wall bound is `activeDeadlineSeconds`, plus deletion 60 s after it (design 28)."
- `crates/substrate-kubernetes/src/exec.rs` reads `DeadlineExceeded` only to choose `Cancelled` for
  a container already terminated; nothing deletes a Pod past the bound.
- `activeDeadlineSeconds` is enforced by the kubelet, so a Pod pinned to a node whose kubelet never
  starts it (an unready node) stays `Pending` and the exec stays `running`.

## Acceptance

- When an exec is observed past its wall bound plus 60 s, measured on the driver's injected wall
  clock, without its container shown terminated, the driver deletes the Pod with its uid
  precondition and ends the exec with the outcome design 28 names for a wall-bound end.
- A foreign-uid or replaced Pod under the name is not deleted.
- The red case above passes; each new case runs once, in seconds, on the fake clock.

## Design

Design 34 § 9 and design 28 (accepted). No contract change.

## Scheduling

Implemented within `wave/k8s-namespace-1` (operator, 2026-10-29: validated and confirmed adversary
findings get a story and are implemented within the current wave). Fix unit R.
