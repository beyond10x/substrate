---
format: aep.planning-md/3
id: story:read-only-mount-sources-refuse-the-daemons-private-paths
kind: story
status: active
title: A read-only root or file is refused when its source is a path the daemon keeps private
summary: The private-path rule of writable roots also holds for read-only roots and read-only files
tags:
- defect
- security
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T02:43:51Z", actor: "human:timo", revision: 2, executor: "agent:substrate"}
- {from: "proposed", to: "active", at: "2026-11-22T02:43:51Z", actor: "human:timo", revision: 3, executor: "agent:substrate"}
---
# Story: a read-only root or file is refused when its source is a path the daemon keeps private

## Outcome

A start that names, as the source of a read-only root (ADR 0010) or a read-only file (ADR 0033),
a path the daemon keeps private is refused by name before dispatch and recorded as refused. The
paths are the ones a writable root (ADR 0038) is already refused for: the daemon's state, the
workspaces of the deployment, its key material.

## What changes

1. The private-path check of the writable roots is applied to both lists at admission.
2. The refusal is named. Which code it carries is decided in the design note.

## Design

Owed before code (invariant 8): a note in design 04 or an amendment to ADR 0010 and ADR 0033
that states the rule, the refusal code, and whether a published schema or vector changes. A
refusal code that is new on the wire adds a successor bundle.

## Acceptance

- An admission unit case: a start naming such a source is refused before dispatch; red before
  the change.
- A toolchain directory outside the private set still mounts and is read; the delegated lane's
  read-only-root cases stay green.

## Not in this story

Which sources a caller may name at all: `story:read-only-mount-sources-are-declared-by-the-operator`.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 1 (high priority), the containment. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
