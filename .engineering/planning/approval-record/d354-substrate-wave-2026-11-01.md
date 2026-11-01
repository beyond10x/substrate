---
format: aep.planning-md/3
id: approval-record:d354-substrate-wave-2026-11-01
kind: approval-record
status: approved
title: 'D354: substrate wave 2026-11-01, four stories whose surfaces do not collide'
relations:
- decides: story:namespace-no-egress-observed-on-every-node
- decides: story:driver-names-a-bare-exec-refusals-verb
- decides: story:listener-held-during-install-is-never-inherited-by-a-fork
- decides: story:pod-sweep-replays-from-the-policys-create
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T15:13:33Z", actor: "human:timo", revision: 2}
---
# D354: substrate wave 2026-11-01, four stories whose surfaces do not collide

## Approval

b10x-chef-orchestrator, 2026-11-01, D354 (class C, under the grant): "lead substrate wave D354. It
is approved under the grant (orchestrator decision D354, 2026-11-01). Record it in the substrate
store as an approval-record before acting." Scope given: "Choose up to 4 units whose surfaces do
not collide, from stories in the store that are proposed, draft or accepted and not implemented",
"Run the aep story-scoper first, then accept the chosen stories under D354." Delivery rule
(operator, 2026-10-28, quoted by the orchestrator): "work in waves or batches ... we run targeted
tests only for what we're currently implementing, and only in the end, when everything ended up in
the integration branch, we run a local full gate and then we create the PR and auto merge it to the
default main branch."

## Units

Integration branch `wave/2026-11-01-d354`, from `main` at `bf0e0443` (0.18.1). One story-scoper
each; the surfaces are the stories' typed scope entries. `CHANGELOG.md` is written by the
coordinator only.

| unit | story | surface |
|---|---|---|
| U1 | `story:namespace-no-egress-observed-on-every-node` (D338 row 1) | `crates/substrate-kubernetes/src/namespace/probe.rs`, `probe_tests.rs`, `src/exec.rs`, `exec_tests.rs`, `restart_tests.rs`, `lib.rs` (inferred) |
| U2 | `story:driver-names-a-bare-exec-refusals-verb` | `crates/substrate-kubernetes/src/namespace/workspace.rs`, a new `namespace/verb_review_tests.rs`, `crates/substrate-kubernetes-client/src/api.rs`, `fake.rs`, `client_tests.rs`, `xtask/src/namespace_lane_run.rs`, design 34 |
| U3 | `story:listener-held-during-install-is-never-inherited-by-a-fork` | `crates/substrate-host/src/egress.rs` |
| U4 | `story:pod-sweep-replays-from-the-policys-create` | `crates/substrate-host/src/confinement/pod.rs`, `pod_cleanup_never_created.rs`, `pod_cleanup_tests.rs`, design 28 |

Ride-along: the ESS pin moves from 0.77.0 to 0.80.0, the newest ESS release (workspace rule: a
wave that touches the repository anyway moves its pin): `spec/**` `requires` lines, the generated
type headers, `xtask/src/ess.rs`, `gate.yml` `ESS_VERSION`/`ESS_SHA256`, `STATUS.md`. Store-only
commit `1b531f35` (`approval-record:d353-substrate-0181-release-recovery`) merged at `54b30150`.

## Selection

Path: computed. `aep plan artifact waves --kind story --status proposed` (aep 0.78.0), after the
scope entries above were written:

- waves: wave 1 = `story:docker-driver-entry-gate`, `story:driver-names-a-bare-exec-refusals-verb`,
  `story:listener-held-during-install-is-never-inherited-by-a-fork`,
  `story:namespace-no-egress-observed-on-every-node`, `story:pod-sweep-replays-from-the-policys-create`
- collisions: none (with `CHANGELOG.md` in each scope the verb returned the six pairs on
  `CHANGELOG.md` only; it was taken out because the coordinator writes it)
- unassessed: `story:docker-image-backed-workload-slice`, `story:docker-workspace-and-exec-slice`,
  `story:firecracker-entry-and-boot-artifact-gate`, `story:firecracker-workspace-and-exec-slice`,
  `story:namespace-lane-runs-exec-on-an-enforcing-network-plugin`,
  `story:namespace-lane-same-named-case-creates-its-workspace`,
  `story:namespace-probe-observes-every-keeper-node`, `story:namespace-probe-repeats-and-sticks-on-egress`

Coordinator decisions:

- U1 binds the probe's keeper node in the driver beside the published snapshot, not in the
  snapshot digest: design 34's 2026-11-01 amendment says the node is not a wire field and the
  refusal (`exec.sandbox-unavailable`, 501) is the guarantee. U1 does not edit design 34.
- U2's Acceptance names "Design 30"; design 30 names no driver API call (scoper, cited), so the
  extra call is stated in design 34. U2's red cases go in a new module so `probe_tests.rs` stays U1's.
- `story:exec-end-after-await-never-panics` (row 13) and `story:pod-orphaned-policy-is-swept`
  (row 18) are already `implemented` (wave D339); not units.
- Left out: `story:docker-driver-entry-gate` (Docker driver entry, not a fix; outside this item);
  the unassessed stories above (not candidates of D354).

## Commits this approval covers

The opening store commit; per unit its commits in its tree and one bot merge into the integration
branch; adversary fix commits and review-result records; the ESS pin commit; the release-prepare
commit (0.19.0: U1 adds a refusal and U2 a review call); the closing store commit; one bot PR to
`main`, merged on green through the bot; after the orchestrator's "lanes go", the annotated tag
through the bot.
