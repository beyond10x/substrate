---
format: aep.planning-md/3
id: story:daemon-holds-the-baseline-objects-of-a-git-workspace
kind: story
status: draft
title: The daemon holds the baseline objects of a Git workspace itself
summary: The changes observation reads the baseline from a daemon-owned store and opens nothing in the workspace's object store
tags:
- security
relations:
- depends_on: story:git-changes-reader-reads-only-the-admitted-workspace
revision: 3
---
# Story: the daemon holds the baseline objects of a Git workspace itself

## Outcome

The changes observation of a Git workspace reads the baseline's commit, trees and blobs from a
store the daemon owns and no writer of the workspace can change. It opens nothing in the
workspace's own object store.

## Where the requirement comes from

Left open by `story:git-changes-reader-reads-only-the-admitted-workspace`, whose design text
states the limit: its check of the workspace's object store holds while no program runs in the
workspace. Stakeholder: the operator.

## Design

Design 21, § *The baseline store* (amendment of 2026-11-22), written and reviewed before the
code. It decides:

- where the store lies: `.substrate-git-baselines/.objects/<root name>/`, one pack and its index;
- when it is written: at create, after the shape check and the checkout, before the commit-id
  file;
- its cost and bound: the compressed size of one commit's files, at most the create's byte limit,
  outside the workspace's reported usage and project quota (D18, taken on default);
- workspaces that exist already: **no store is built for them** (D20, the operator's decision,
  replacing his D19). Both Git observations of such a workspace are refused
  `workspace.git-baseline-missing` until it is created again;
- the shape of a baseline, checked before anything walks it: nesting, names and path lengths,
  object sizes, path count and summed bytes;
- the stat manifest of ADR 0051: not part of this step.

Design 23 holds unchanged: a driver with no Git source configured touches nothing in
`.substrate-git-baselines`.

## Acceptance

With a program running in the workspace and writing its repository metadata, an observation
returns only bytes and sizes of the daemon-held baseline and of files beneath the workspace root,
and does not wait on anything the program made.

## What was built

Pull request 261. Reviews: `review-result:baseline-store-design-read`,
`review-result:baseline-store-security-1` to `-4`. The decisions are in
`approval-record:decisions-on-the-baseline-store-2026-11-22` and
`approval-record:operator-picks-no-store-build-for-older-git-workspaces-2026-11-22`.

## For the release that ships it

The changelog names the effect of an upgrade under Changed: a Git workspace created by an
earlier release has no Git observation until it is created again. The operator asked for it to
be named in the release notes.
