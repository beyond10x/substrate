---
format: aep.planning-md/3
id: approval-record:wave-2026-11-04-substrate
kind: approval-record
status: approved
title: 'Substrate wave 2026-11-04: three accepted stories and one spike'
relations:
- decides: story:failed-get-review-keeps-the-create-denial
- decides: story:abandoned-forwarder-reaped-through-eintr
- decides: story:namespace-lane-same-named-case-creates-its-workspace
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-04T07:33:32Z", actor: "human:timo", revision: 2}
---
# Substrate wave 2026-11-04: three accepted stories and one spike

## Approval

b10x-chef-orchestrator, 2026-11-04, under the grant (class C): "Accept P1, P2 and P3 as U2, U1 and
U3." "U4, the kind-plus-Calico spike for P4 and P5, goes ahead: local images only, the result goes
into P4 and P5's bodies, nothing accepted from it." "P4, P5, P6, the Docker stories and the
Firecracker stories and epic stay as they are." "Release 0.19.2; the pin stays at 0.81.0." Builds
and kind clusters start only on the build terms the orchestrator relays.

Delivery rule (operator, 2026-10-28): "work in waves or batches ... We run targeted tests only for
what we're currently implementing, and only in the end, when everything ended up in the integration
branch, we run a local full gate and then we create the PR and auto merge it to the default main
branch."

## Units

Integration branch `wave/2026-11-04-substrate`, from `main` at `a692acb7` (0.19.1).

| unit | story | surface |
|---|---|---|
| U1 | `story:failed-get-review-keeps-the-create-denial` | `crates/substrate-kubernetes/src/namespace/workspace.rs`, `namespace/verb_review_tests.rs`, design 34 § 7.1 |
| U2 | `story:abandoned-forwarder-reaped-through-eintr` | `crates/substrate-host/src/egress.rs` |
| U3 | `story:namespace-lane-same-named-case-creates-its-workspace` | `xtask/src/namespace_lane_run.rs` (step 6) |
| U4 | spike for `story:namespace-lane-runs-exec-on-an-enforcing-network-plugin` and `story:namespace-probe-repeats-and-sticks-on-egress` | no source: a local kind cluster with a pinned Calico, a default-deny Pod, the errno of one connect attempt; recorded in both stories' bodies |

No two units share a file. `CHANGELOG.md` is written by the coordinator only. U2 gets a security
pass beside its adversary pass (confinement code).

## Store-only

`story:namespace-lane-exec-create-case-reads-the-apis-bare-refusal` moved to implemented without
code (`da3c69f7`): delivered by `3413131d` (0.18.1), refined by `ec461246` and `79b7fb0d` (0.19.0);
the 0.19.0 and 0.19.1 tags carry Namespace-lane records.

`story:kubernetes-namespace-workspace-and-exec-slice` stays active: its exec cases on the namespace
lane wait for the two stories U4 informs.

## Commits this approval covers

The store commits; per unit its commits in its tree and one bot merge into the integration branch;
review fix commits and review-result records; the release-prepare commit for 0.19.2; the closing
store commit; one bot PR to `main`, merged on green through the bot; after the release lanes, the
annotated tag through the bot and the release `release.yml` publishes.
