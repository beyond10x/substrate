---
format: aep.planning-md/3
id: story:linked-worktree-git-common-dir
kind: story
status: draft
title: A linked worktree reaches its Git common directory
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: a linked worktree reaches its Git common directory

## Problem

A linked Git worktree's `.git` is a file naming `gitdir: <common dir>/worktrees/<name>`, an absolute
host path outside the workspace. Inside a confined exec that path does not exist and `git status`
exits 128.

## Design

ADR 0010 already admits a declared host directory read-only at a declared mount point. Declaring the
repository's common directory as a read-only root mounted at its own host path makes the `gitdir:`
line resolve without a new contract. A writable mount of a host directory outside the workspace is
outside ADR 0010 ("Nothing is silently dropped, re-pointed or made writable") and is not part of
this story.

## Acceptance

- A runtime test adopts a linked worktree, declares its common directory read-only at the same
  path, and `git status` and `git log -1` exit 0 inside the exec.
- The same exec cannot create a file in the common directory.
- The public documentation names this as the way to run Git in a linked worktree, and says that
  Git commands that write the index or refs are refused.
