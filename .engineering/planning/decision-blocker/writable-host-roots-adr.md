---
format: aep.planning-md/3
id: decision-blocker:writable-host-roots-adr
kind: decision-blocker
status: cleared
title: Accept the writable-host-roots ADR
relations:
- blocks: story:writable-host-roots
revision: 4
transitions:
- {from: "open", to: "cleared", at: "2026-10-22T10:47:48Z", actor: "agent:b10x-chef-orchestrator", revision: 4}
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

## Answer

Operator, 2026-10-22, relayed verbatim by the orchestrator: "either name physical paths, or provide
git source". Orchestrator: "So a writable root outside /workspace is one of two kinds: (1) a host
path the operator names physically in the daemon config, or (2) a git source the run gets as a
writable checkout. Write the ADR on that design (it replaces options A/B/C; B's named, fd-bound
selection fits kind 1)".

Decided by `adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md` (accepted), recorded
as `architecture-decision-record:writable-host-roots-are-operator-declared`.
