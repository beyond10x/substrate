---
format: aep.planning-md/3
id: approval-record:wave-2026-10-30-remote-conformance
kind: approval-record
status: active
title: 'Wave 2026-10-30: remote clean-room conformance and three namespace-driver follow-ups'
relations:
- decides: story:remote-clean-room-conformance
- decides: story:kubernetes-collisions-metric
- decides: story:preconditioned-delete-409-is-not-a-uid-mismatch
- decides: story:unconfirmed-pod-deletion-ends-unknown
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 3}
---
# Wave 2026-10-30: remote clean-room conformance and three namespace-driver follow-ups

Integration branch `wave/2026-10-30-remote-conformance`, base `80935a49` (main after release
0.16.1). Targeted tests per unit, one local gate scoped to the touched crates at the end, one pull
request through the bot, then release 0.17.0 through `AGENTS.md` § *Releases*.

## Approval

b10x-chef-orchestrator, 2026-10-30, D289, under the grant in the collection's session
instructions: "Approved as D289 (decisions.md): wave 2026-10-30-remote-conformance, units u1-u4.
u2-u4 go draft->proposed->active. ESS pin to 0.77.0, release 0.17.0. Start now and record the
approval in your store." On the design 30 blocker: "option A accepts an ADR, which is on the
never-list ... I present it to Timo, and the wave does not wait on it."

## Selection

`aep plan artifact waves --status proposed` (aep 0.78.0), verbatim:

```
wave 1
  story:docker-driver-entry-gate (inferred)
  story:remote-clean-room-conformance (inferred)
unassessed: story:docker-image-backed-workload-slice
unassessed: story:docker-workspace-and-exec-slice
unassessed: story:firecracker-entry-and-boot-artifact-gate
unassessed: story:firecracker-workspace-and-exec-slice
1 wave(s), 0 collision(s), 4 unassessed
```

`aep plan artifact waves --status draft`, verbatim:

```
wave 1
  story:host-process-adopt (inferred)
  story:kubernetes-collisions-metric (inferred)
  story:local-live-kill-case-waits-for-queued-chunk
  story:preconditioned-delete-409-is-not-a-uid-mismatch (inferred)
  story:unconfirmed-pod-deletion-ends-unknown (inferred)
wave 2
  story:host-process-container-records (inferred)
collision: story:host-process-adopt story:host-process-container-records crates/substrate-daemon/src/app/processes.rs (inferred)
collision: story:host-process-adopt story:host-process-container-records crates/substrate-daemon/tests/runtime_vectors.rs (inferred)
collision: story:host-process-adopt story:host-process-container-records crates/substrate-host/src/host_process
collision: story:host-process-adopt story:host-process-container-records crates/substrate-store/src/host_processes.rs (inferred)
collision: story:host-process-adopt story:host-process-container-records crates/substrate-wire/src/host_process_tests.rs
collision: story:host-process-adopt story:host-process-container-records crates/substrate-wire/src/lib.rs
collision: story:host-process-adopt story:host-process-container-records scripts/gate.sh (inferred)
collision: story:host-process-adopt story:host-process-container-records spec/host-processes
collision: story:host-process-adopt story:host-process-container-records xtask/bundle-source/0.23.0 (inferred)
unassessed: story:confinement-adapter-on-the-wire
unassessed: story:docs-shared-site
unassessed: story:pod-orphaned-policy-is-swept
2 wave(s), 9 collision(s), 3 unassessed
```

Selection by the verb plus the scheduling the coordinating session already recorded in the
stories: D186 puts `story:remote-clean-room-conformance` in the first wave after
`wave/k8s-namespace-1`, D187 puts `story:kubernetes-collisions-metric` beside it, and the two
Kubernetes fix stories name the same wave. Scope entries for the four were written on 2026-10-30
from design 31 § 10 (cited) and the paths the stories cite; the rest are marked inferred.

## Units

| unit | story | lands on |
|---|---|---|
| u1 | `story:remote-clean-room-conformance` | new crate `crates/b10x-substrate-conformance/`, workspace members and `Cargo.lock`, `xtask/src/packages.rs`, `scripts/delegated-lane.sh`, a guide page under `website/docs/guides/` with `website/sidebars.ts`; then the chart-lane step (`Dockerfile`, `.github/workflows/chart-lane.yml`, `xtask/src/chart_cluster.rs`) and namespace-lane step 4 (`xtask/src/namespace_lane_run.rs`) |
| u2 | `story:kubernetes-collisions-metric` | `crates/substrate-daemon/src/app/health.rs`, the driver's collision counter, the operational-listener goldens |
| u3 | `story:preconditioned-delete-409-is-not-a-uid-mismatch` | `crates/substrate-kubernetes-client/src/api.rs` (`remove_once`), its fake |
| u4 | `story:unconfirmed-pod-deletion-ends-unknown` | `crates/substrate-kubernetes/src/exec.rs`, `exec_tests.rs`, one sentence in design 34 |

All four start at once; u1's lane steps follow its runner inside the same unit. Adversary pass on
each unit; security review beside u1 (credential handling, TLS negatives) and u3 (Pod removal under
the uid rule).

Coordinator ride-alongs: the ESS pin 0.76.0 to 0.77.0 (`spec/ess-inputs.yaml`, regenerated
projections, `ESS_VERSION` and `ESS_SHA256` in `gate.yml`), the version 0.17.0, `CHANGELOG.md`,
`AGENTS.md` and `STATUS.md`.

## Left out

- `story:docker-driver-entry-gate`: design 15 is `proposed`; accepting it and numbering its ADR is
  the operator's, and the Docker track opens only after shared remote conformance (u1) is green.
- `story:docker-workspace-and-exec-slice`, `story:docker-image-backed-workload-slice`,
  `story:firecracker-entry-and-boot-artifact-gate`, `story:firecracker-workspace-and-exec-slice`:
  each depends on a story that is not terminal (u1 or the Docker gate); Firecracker also needs an
  accepted design or ADR and a KVM node.
- `story:local-live-kill-case-waits-for-queued-chunk`: a test-timing fix, ready; next wave, cap 4.
- The exec refusal on the namespace driver: no proposed story covers it. It needs design 30
  accepted with an ADR (invariant 8), filed as `decision-blocker:namespace-exec-needs-design-30`.

## Commits this approval authorises

One commit or more per unit in its own tree, the merges into the integration branch with a body,
the coordinator's ride-along and store commits, one pull request to `main` through the bot merged
when `Full gate` is green, and release 0.17.0: the annotated tag with its four lane records,
`verify-release.sh`. Nothing else.
