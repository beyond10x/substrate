---
format: aep.planning-md/3
id: approval-record:operator-picks-no-store-build-for-older-git-workspaces-2026-11-22
kind: approval-record
status: draft
title: 'Operator''s pick: no store is built for a Git workspace that predates the store (2026-11-22)'
relations:
- decides: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Operator's pick: no store is built for a Git workspace that predates the store (2026-11-22)

## Who decided

The operator, on 2026-11-22 at 21:56:48 CET. His pick reached this repository's session as a
relay by session `host-operator`; it was not typed in this session, and it is not a decision of
that session. It replaces his pick of 20:31:47 CET the same day (D19, "A: keep one-time build"),
recorded in `approval-record:decisions-on-the-baseline-store-2026-11-22`.

## The question

D20, put to him through `host-operator` after the fourth security review: Git workspaces created
before the change.

- B (recommended): no build. Such a workspace answers both Git observations
  `workspace.git-baseline-missing` until it is created again.
- A1: keep the build, in a child process with memory and processor limits.
- A0: merge the build as it was, in the daemon's process.

## The words, as relayed

> host-operator relays Timo's pick, 21:56:48 CET, typed in the host-operator session: D20 "B: no
> build". This replaces his D19 A of 20:31. A Git workspace created before the change answers
> both Git observations `workspace.git-baseline-missing` until it is created again; the one-time
> build and its tests leave PR 261. Name the upgrade effect in the release notes when it ships.
> Confirm in one line.

## What follows

The build, its copy, its turn and their tests left pull request 261. The changelog names the
effect of an upgrade under Changed, for the release notes. Not known when this was decided:
whether a deployment holds Git workspaces that live across an upgrade.
