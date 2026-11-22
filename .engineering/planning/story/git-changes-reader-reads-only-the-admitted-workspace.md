---
format: aep.planning-md/3
id: story:git-changes-reader-reads-only-the-admitted-workspace
kind: story
status: active
title: The Git changes reader reads only the admitted workspace
summary: Every byte of a change-set answer comes from the admitted workspace or its daemon-held baseline
tags:
- defect
- security
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T02:16:41Z", actor: "human:timo", revision: 2, executor: "agent:substrate"}
- {from: "proposed", to: "active", at: "2026-11-22T02:16:42Z", actor: "human:timo", revision: 3, executor: "agent:substrate"}
---
# Story: the Git changes reader reads only the admitted workspace

## Outcome

Every byte the changes observation of a Git workspace returns comes from a file beneath the
admitted workspace's root or from the baseline the daemon holds for it. What the workspace's own
repository metadata says decides neither which directory is read nor how a file is read.

## What changes

1. `git::changes` in `crates/substrate-host/src/git.rs` reads worktree files through the guarded
   file system, beneath the workspace root, and compares them with the baseline's objects. It
   does not take the worktree, or the way an entry is read, from the workspace's repository.
2. The per-file byte bound holds for every file the reader opens.
3. Every other reader in that file that opens the workspace's repository is read for the same
   rule, and the result is written in this story.

## Design

No contract, schema or capability fact changes. Design 21 gets the rule in one sentence.

## Acceptance

- Unit cases in `crates/substrate-host/src/git.rs` hold the rule for a workspace whose
  repository metadata was changed by its writer; each is red before the change. Run time under
  1 s each.
- The existing change-set cases of the daemon's vectors stay green.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 2 (high priority). The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
