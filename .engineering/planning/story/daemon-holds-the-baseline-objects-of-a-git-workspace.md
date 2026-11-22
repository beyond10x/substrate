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
revision: 2
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
- when it is written: at create, after the checkout is verified, before the commit-id file;
- its cost and bound: the compressed size of one commit's files, at most the create's byte limit,
  outside the workspace's reported usage and project quota (D18, taken on default: no answer
  from the operator within 15 minutes of the question of 15:11 CET, 2026-11-22);
- workspaces that exist already: the first observation builds the store from a copy of the
  workspace's object store, one build at a time;
- the shape of a baseline: at most 1,024 nested directories and no more paths than the inode
  limit, checked before anything walks it;
- the stat manifest of ADR 0051: not part of this step.

Design 23 holds unchanged: a driver with no Git source configured touches nothing in
`.substrate-git-baselines`.

## Acceptance

With a program running in the workspace and writing its repository metadata, an observation
returns only bytes and sizes of the daemon-held baseline and of files beneath the workspace root,
and does not wait on anything the program made.

## What was built

Pull request 261, commits `db928a20b` and `792963047` (design), `1ec20d788`, `6d5169e4c` and
`dbe1f9092` (code). Reviews: `review-result:baseline-store-design-read`,
`review-result:baseline-store-security-1`, `review-result:baseline-store-security-2`,
`review-result:baseline-store-security-3`.

Not shown by a test: the project quota's byte and inode limits as a build's bounds (they need
the real-quota fixture).
