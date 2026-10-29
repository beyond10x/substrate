---
format: aep.planning-md/3
id: decision-blocker:quota-refusal-is-recognised-inside-forbidden
kind: decision-blocker
status: cleared
title: A quota refusal is recognised inside the Forbidden message (D184)
relations:
- blocks: story:kubernetes-namespace-workspace-and-exec-slice
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-29T05:25:18Z", actor: "human:timo", revision: 2}
---
# Decision: a quota refusal is recognised inside the Forbidden message (D184)

Decided by the orchestrating session on 2026-10-29 as D184, inside design 34, which the operator
accepted.

## Observation

The namespace lane's recorded run (`Namespace-lane: 425ab9c2709b54bc46c234b4646913d8f12f27a2 8
2026-10-29T05:19:33Z`), observation O7: a PVC create over the ResourceQuota answers 403 with the
message `persistentvolumeclaims "…" is forbidden: exceeded quota: substrate-lane, …`. Design 34
§ 10.2 step 6 (line 378) recognises capacity only by a message that starts `exceeded quota:`, so
§ 10.3 would leave capacity as `kubernetes.api-failed`, against the slice's acceptance.

## Decision

A 403 Status with reason `Forbidden` whose message contains `is forbidden: exceeded quota: ` maps to
`workspace.capacity` or `exec.capacity` (429, `exhausted`). Any other 403 stays `Failed`
`kubernetes.api-failed`. The golden is the recorded message. U13 amends § 10.2 step 6 and O7.
