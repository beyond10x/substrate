---
format: aep.planning-md/3
id: story:keeper-check-proves-guarded-io
kind: story
status: active
title: The keeper check proves guarded I/O and atomic replacement before they are published
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:kubernetes-namespace-workspace-and-exec-slice
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T08:50:30Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T08:50:30Z", actor: "human:timo", revision: 3}
---
# Story: The keeper check proves guarded I/O and atomic replacement before they are published

## Outcome

A Kubernetes namespace machine publishes `workspace.guarded-io` and `workspace.atomic-replace` only
after the keeper has shown, in the Pod, that a symlink escape is refused and that a `renameat2`
replacement succeeds, as design 34 § 6.2 requires. Until then both facts are absent (invariant 3).

## Finding

Adversary pass of 2026-10-29 on `wave/k8s-namespace-1` at `661d68e0`, scope namespace, red case
`namespace::probe_tests::a_probe_whose_check_proves_only_openat2_publishes_no_guarded_io_or_atomic_replace`
on `adv/k8s-namespace` (`1d607e8a`), verified by the coordinator:

- `substrate-executor fs check` answers only `openat2_beneath` from `openat2_available()`
  (`crates/substrate-executor/src/serve.rs`, `Request::Check`).
- The probe sets `workspace_guarded_io`, `workspace_openat2_beneath` and
  `workspace_atomic_replace` from that one bit (`crates/substrate-kubernetes/src/namespace/probe.rs`,
  the snapshot builder).
- Design 34 § 6.2's row for the keeper check names three proofs: `openat2` with the four resolve
  flags, a symlink escape refused, a `renameat2` replacement; each fact comes from its own proof.

## Acceptance

- The executor's check runs the three proofs in a scratch directory inside the workspace mount and
  answers each one separately; nothing it creates outlives the check.
- The probe publishes `workspace.openat2-beneath` from the first, `workspace.guarded-io` only when
  the first two hold, and `workspace.atomic-replace` only from the third.
- The red case above passes; the existing probe assertions that expected `Some(true)` from the
  openat2 bit alone are corrected to drive the check's new answers, not weakened.
- Executor tests show each proof failing on its own leaves its fact absent.

## Design

Design 34 § 6.2 (accepted). No contract change: the executor protocol is internal to the driver, and
the three facts already exist in bundle `0.25.0`.

## Scheduling

Implemented within `wave/k8s-namespace-1` (operator, 2026-10-29: validated and confirmed adversary
findings get a story and are implemented within the current wave).
