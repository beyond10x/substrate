---
format: aep.planning-md/3
id: story:read-only-mount-sources-are-declared-by-the-operator
kind: story
status: draft
title: Read-only mount sources are declared by the operator and granted per caller
summary: A caller names a declared source; the daemon resolves it beneath operator-owned roots
tags:
- security
relations:
- depends_on: story:read-only-mount-sources-refuse-the-daemons-private-paths
revision: 1
---
# Story: read-only mount sources are declared by the operator and granted per caller

## Outcome

A caller names a source the operator declared, as a writable root is declared today (ADR 0038).
The daemon resolves the name beneath operator-owned roots and admits it only for a caller it is
granted to. A host path the operator did not declare is refused by name before dispatch.

## Design

Owed before code: an ADR that amends ADR 0010, which makes the source caller-selected, and
ADR 0033. It is a change of contract and capability: the start fields, the facts
`exec.read-only-roots` and `exec.read-only-files`, the refusals, a successor bundle, and what
consumers that pass host paths today do instead. The ADR names those consumers.

## Open

- Whether the local posture, where the caller is the daemon's own uid, keeps caller-selected
  paths. The ADR decides.
- This is a change of architecture and waits for the operator's word on the ADR.

## Acceptance

An approved toolchain source mounts for a caller it is granted to; an undeclared path and a
declared source not granted to the caller are each refused before dispatch, by name.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 1 (high priority), the durable fix. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
