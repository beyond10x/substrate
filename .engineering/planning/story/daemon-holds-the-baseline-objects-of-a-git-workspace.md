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
revision: 1
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

Owed before code: an amendment to design 21, or an ADR. It decides where the store lies, how
materialization fills it, what it costs in storage per workspace and how that is counted against
quotas, what happens to workspaces that exist already, and whether the stat manifest of ADR 0051
replaces the per-observation hashing in the same step.

## Acceptance

With a program running in the workspace and writing its repository metadata, an observation
returns only bytes and sizes of the daemon-held baseline and of files beneath the workspace root,
and does not wait on anything the program made.
