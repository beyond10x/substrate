---
format: aep.planning-md/1
id: story:full-review-git-recovery
kind: story
status: active
title: The full-review work starts from a verified Git database
summary: Corrupt local objects and unmerged worktrees are recovered without losing user work.
relations:
- decomposes: epic:release-hardening
revision: 3
---
# Story: Git and worktree recovery

## Outcome

The repository has a clean object database; the relevant contract work is recovered; PTY remains a separate valid worktree; and no corrupt backup is deleted.

## Acceptance

- `git fsck --full` passes.
- The contract-gate filesystem-only change is reconstructed and tested.
- PTY retains its twelve commits and all pre-existing dirt in a separate worktree.
- Recovery archives have SHA-256 manifests.
