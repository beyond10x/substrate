---
format: aep.planning-md/3
id: story:writable-host-roots
kind: story
status: draft
title: A confined child writes a declared host directory outside /workspace
revision: 2
---
# A confined child writes a declared host directory outside /workspace

takt's harness session needs to write its working directory, decision channel and config home,
and git needs to write a linked worktree's common git directory, all outside the workspace
(takt `dependency-blocker:loom-child-confinement`, G5). ADR 0023, ADR 0010 and ADR 0033 each say the
workspace is the only writable path, so this needs a new ADR:
`architecture-decision-record:writable-host-roots-are-operator-declared`, proposed. Nothing is built
until it is accepted.
