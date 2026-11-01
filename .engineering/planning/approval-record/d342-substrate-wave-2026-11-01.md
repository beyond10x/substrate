---
format: aep.planning-md/3
id: approval-record:d342-substrate-wave-2026-11-01
kind: approval-record
status: approved
title: 'D342: the refused executor handshake and the chart-lane ETXTBSY flake'
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T05:07:20Z", actor: "human:timo", revision: 2}
---
# D342: substrate wave 2026-11-01, the refused executor handshake and the chart-lane ETXTBSY flake

## Approval

b10x-chef-orchestrator, 2026-11-01, D342 (class C, under the grant): "your proposal A. Red
FakeCluster case first (403 Status body on the exec upgrade), then the handshake refusal carries the
API's text and fails at once on 403; the lane keeps a failed case's daemon log. Lane-only relaxation
declined. Version stays 0.18.1: it was never tagged, so no new release-prepare; add the fix to the
CHANGELOG [0.18.1] section in the wave. story:chart-lane-stand-ins-never-fail-with-etxtbsy may ride
as unit 2 if its surface does not collide."

## Units

Integration branch `wave/2026-11-01-d342`, on `main` at `669c5bb8` (the 0.18.1 release prepare).

| unit | story | surface |
|---|---|---|
| U1 | `story:refused-executor-handshake-names-the-api-answer` | `crates/substrate-kubernetes-client/src/exec.rs`, `crates/substrate-kubernetes/src/namespace/workspace.rs`, probe tests, `xtask/src/namespace_lane_run.rs` (log retention) |
| U2 | `story:chart-lane-stand-ins-never-fail-with-etxtbsy` | `xtask/src/chart_cluster.rs` |

The surfaces do not collide: U2 touches only `chart_cluster.rs`. `CHANGELOG.md` `[0.18.1]` is
shared.

## Why

The 0.18.1 namespace lane at `669c5bb8` gave no record on "S6 Role without pods/exec create"; the
lane rule tightened by D339 U2 found no `WARN` line carrying the API's refusal. Main's Chart lane
run 41271743601 failed once more on the ETXTBSY flake (second hit). The tag 0.18.1 goes on this
wave's merge commit after the four lanes.
