---
format: aep.planning-md/3
id: story:full-review-git-recovery
kind: story
status: implemented
title: The full-review work starts from a verified Git database
summary: Corrupt local objects and unmerged worktrees are recovered without losing user work.
relations:
- decomposes: epic:release-hardening
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T08:37:43Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T08:37:43Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:49:57Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Story: Git and worktree recovery

## Outcome

The repository has a clean object database; the relevant contract work is recovered; PTY remains a separate valid worktree; and no corrupt backup is deleted.

## Acceptance

- `git fsck --full` passes.
- The contract-gate filesystem-only change is reconstructed and tested.
- PTY retains its twelve commits and all pre-existing dirt in a separate worktree.
- Recovery archives have SHA-256 manifests.
