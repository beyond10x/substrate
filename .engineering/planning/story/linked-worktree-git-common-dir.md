---
format: aep.planning-md/3
id: story:linked-worktree-git-common-dir
kind: story
status: implemented
title: A linked worktree reaches its Git common directory
relations:
- decomposes: epic:host-run-gates
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-10T00:48:48Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-10T00:48:48Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-10T00:48:59Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
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

## Evidence

Implemented by commit `dde2a812b` (merged in https://github.com/beyond10x/substrate/pull/122 as
`a16b12b20`) and released in
[0.7.12](https://github.com/beyond10x/substrate/releases/tag/0.7.12), with no contract change:
the common directory is an ADR 0010 read-only root mounted at its own host path.

- First criterion: `git_status_and_log_run_in_a_linked_worktree_with_its_common_dir_read_only`
  (`crates/substrate-host/tests/linked_worktree_git_common_dir.rs`), beside the negative control
  `git_status_in_a_linked_worktree_fails_without_its_common_dir`.
- Second criterion: `an_exec_cannot_write_into_the_declared_common_dir` (same file).
- Third criterion: `website/docs/concepts/confinement.md` names the read-only common-directory
  root as the way to run Git in a linked worktree and says writes to the index or refs are refused.
- These cases run in the delegated lane only (`bash scripts/delegated-lane.sh`); the release's
  delegated confinement record is 97 cases at `7d268e592`
  (https://github.com/beyond10x/substrate/releases/tag/0.7.12).
- Full gate green on the merge commit:
  https://github.com/beyond10x/substrate/actions/runs/38001895521.
- A writable common directory stays out of scope; a single read-only host file is
  story:read-only-host-file-bind.
