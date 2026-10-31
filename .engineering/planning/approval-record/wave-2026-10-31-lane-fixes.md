---
format: aep.planning-md/3
id: approval-record:wave-2026-10-31-lane-fixes
kind: approval-record
status: active
title: 'Wave 2026-10-31: the two lane failures that block release 0.17.0'
relations:
- decides: story:force-deleted-pod-cases-wait-for-the-answer
- decides: story:clean-room-destroy-settles-on-the-namespace-driver
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-30T23:39:43Z", actor: "human:timo", revision: 2}
---
# Wave 2026-10-31: the two lane failures that block release 0.17.0

Integration branch `wave/2026-10-31-lane-fixes`, base `28bf24fe` (main after PR 199). Targeted
tests per unit, one local gate at the end, one pull request through the bot; after its merge the
four release lanes run on the merge commit and release 0.17.0 is tagged through `AGENTS.md`
§ *Releases*.

## Approval

b10x-chef-orchestrator, 2026-10-31, D312, under the grant in the collection's session
instructions: "Run one fix wave of 2 units on one integration branch, then one PR: U1 Confinement:
fix the test race at `pod_cleanup_force_deleted.rs:155`. Mark the Pod gone only after the fake has
answered, so a watch is created. U2 Namespace: first use aep:diagnosing to find why
`workspace.destroy` and `events.commit-order` return `operation.outcome-unknown`. Then fix it, and
make sure leaked workspaces are cleaned so S5 and S6 do not hit the 4-PVC quota (429). If no cause
can be found, report the evidence and do not guess." Stop rule: the same cause failing twice more
stops the wave.

## Units

- U1 `story:force-deleted-pod-cases-wait-for-the-answer`
- U2 `story:clean-room-destroy-settles-on-the-namespace-driver`
