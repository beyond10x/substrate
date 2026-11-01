---
format: aep.planning-md/3
id: approval-record:d339-substrate-wave-2026-11-01
kind: approval-record
status: active
title: 'D339: substrate wave 2026-11-01, four stories whose surfaces do not collide'
relations:
- informed_by: approval-record:d338-review-findings-decided-in-session
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-01T00:37:38Z", actor: "human:timo", revision: 2}
---
# D339: substrate wave 2026-11-01, four proposed stories whose surfaces do not collide

## Approval

b10x-chef-orchestrator, 2026-11-01, D339 (class C, under the grant): "next substrate wave. Pick up
to 4 of the 6 proposed stories whose surfaces do not collide (design 34 amendment before row 1's
code; rows 13, 18; the 3 D338 stories), accept them, open the integration branch carrying
plan/2026-11-01-d338-adoptions, and do the no-cargo work now (scoping, design 34 amendment,
approval-record). Start no cargo until I tell you the ESS D335 final gate is done (one build slot).
Waves rule: targeted tests per unit, one full gate at the end, one bot PR merged on green, one
release."

## Units

Integration branch `wave/2026-11-01-d339`, on `plan/2026-11-01-d338-adoptions`. Scoped by one
story-scoper each; the surfaces below are the stories' Scope entries.

| unit | story | surface |
|---|---|---|
| U1 | `story:probe-withheld-facts-name-the-failed-calls-code` | `crates/substrate-kubernetes/src/namespace/probe.rs`, `probe_tests.rs` |
| U2 | `story:namespace-lane-verb-case-accepts-only-its-own-refusal` (depends on U1) | `xtask/src/namespace_lane_run.rs` |
| U3 | `story:exec-end-after-await-never-panics` | `crates/substrate-kubernetes/src/exec.rs`, `exec_tests.rs` |
| U4 | `story:pod-orphaned-policy-is-swept` | `crates/substrate-host/src/confinement/pod.rs`, `crates/substrate-kubernetes-client/src/gone.rs`, `crates/substrate-host/tests/adversary_pod.rs` |

Constraints from the scoping:

- U2 needs U1's API messages for `pods get`, `pods delete` and `persistentvolumeclaims get`; U1
  lands first. U2 is lane-only and decides after the first probe has finished.
- U4 keeps its rule in a new sweep-only function: no `PodEnd` variant and no change to how
  `decide` reads `BOOKMARK`, because the namespace driver's exec watch (`exec.rs`, U3's file)
  matches `PodEnd` and calls `quoted_watch_event`. Whether a watch replay can be shown complete
  with the Role's `watch` verb alone is observed on kind before its design is chosen.
- `CHANGELOG.md` `[Unreleased]` is shared by every unit.

## Design 34 amendment, before row 1's code

`story:namespace-no-egress-observed-on-every-node` (the 2026-10-31 sheet's row 1) is not a unit of
this wave: its code shares `probe.rs` with U1 and `exec.rs` with U3. Its design 34 amendment lands
in this wave (amendments of 2026-11-01): an exec start whose workspace keeper is on a node the
probe did not observe is refused `exec.sandbox-unavailable` before the Pod create. Probing every
schedulable node was not chosen, because ADR 0047 decision 3 gives the driver no `nodes` access;
probing each live keeper's node is `story:namespace-probe-observes-every-keeper-node`. Row 1's
Acceptance is rewritten for the refusal.

## Left for a later wave

- `story:namespace-no-egress-observed-on-every-node` (code; amendment done here).
- `story:namespace-lane-same-named-case-creates-its-workspace` (shares `xtask/src/namespace_lane_run.rs` with U2).
- `story:namespace-probe-observes-every-keeper-node` (after row 1).
