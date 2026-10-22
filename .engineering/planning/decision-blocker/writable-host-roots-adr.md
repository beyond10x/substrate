---
format: aep.planning-md/3
id: decision-blocker:writable-host-roots-adr
kind: decision-blocker
status: open
title: Accept the writable-host-roots ADR
relations:
- blocks: story:writable-host-roots
revision: 2
---
# Decision blocker: accept the writable-host-roots ADR

## Question

A writable mount outside `/workspace` contradicts ADR 0023 § Consequences, ADR 0010 and ADR 0033.
It needs a new ADR, which is the operator's.

| option | what | cost |
|---|---|---|
| A | mount more adopted workspaces (ADR 0035) writably at named mounts | cannot reach a directory outside the workspace root; takt's common git dir and config home stay unwritable |
| B | operator-declared writable roots in daemon config, selected by name, bound by fd | new host write authority; one new ADR and bundle |
| C | nothing; Loom confines only `aep` and git | takt's forge and harness session stay unconfined |

Recommendation: B, drafted as
`architecture-decision-record:writable-host-roots-are-operator-declared` (proposed).

## Record

Orchestrator, 2026-10-22: "Agreed: ADR is Timo's; I asked him A/B/C." Then: "Accepted: draft B
(proposed, not accepted, no implementation); adr/0035:18 confirms the single-component limit. I put
B to Timo as the recommendation."
