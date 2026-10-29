---
format: aep.planning-md/3
id: story:exec-starts-only-on-bound-pvc
kind: story
status: implemented
title: An exec starts only on the workspace's bound PVC
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:kubernetes-namespace-workspace-and-exec-slice
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T08:55:29Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T08:55:29Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-29T12:18:56Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: An exec starts only on the workspace's bound PVC

## Outcome

Before it creates an exec Pod, the driver reads the workspace's PVC and refuses the start when the
name no longer holds the bound uid, so an exec never mounts a same-named replacement claim (design 34
§ 4 uid rule).

## Finding

Adversary pass of 2026-10-29 on `wave/k8s-namespace-1` at `661d68e0`, scope exec, red case
`exec_tests::a_replaced_workspace_pvc_refuses_the_start_before_any_pod` on `adv/k8s-exec`
(`41e746e7`), verified by the coordinator:

- `placement` in `crates/substrate-kubernetes/src/exec.rs` GETs the keeper and compares its uid, but
  takes the PVC uid from the ledger without reading the PVC; the exec Pod mounts the claim by name.
- Reaching it needs the keeper terminal but still present while the PVC is deleted and recreated
  under the same name; narrow, and the uid rule applies to every object addressed by name.

## Acceptance

- `placement` reads the PVC and refuses with the same unplaced refusal it uses for a replaced keeper
  when the PVC is absent or holds another uid; no Pod is created.
- The red case above passes.

## Design

Design 34 § 4 (accepted). No contract change.

## Scheduling

Implemented within `wave/k8s-namespace-1` (operator, 2026-10-29). Fix unit R.
