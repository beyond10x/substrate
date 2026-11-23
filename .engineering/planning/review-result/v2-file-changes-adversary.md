---
format: aep.planning-md/3
id: review-result:v2-file-changes-adversary
kind: review-result
status: active
title: Adversary pass on the v2 changes of one file (pull request 274)
relations:
- reviews: story:file-change-diff-is-a-unified-diff-git-applies
- reviews: story:line-patch-changes-only-the-lines-it-names
- reviews: story:exact-text-edit-refuses-overlapping-matches
- reviews: story:failed-file-replacement-removes-the-directories-it-created
revision: 1
---
# Adversary pass: the v2 changes of one file (pull request 274)

Pass on the four stories of pull request 274, on the working tree of
`fix/v2-file-mutations-review-0-24-0` over `main` at `ca9cdc404`: the line patch, the exact-text
edit, the diff a result carries and the cleanup after a failed replacement, all in
`crates/substrate-guarded/src/fs.rs`. Verdict: BROKEN, two confirmed warnings, no blocker.

Counts as reported: the guarded crate's cases 87 to 98, the reviewer's 11 among them, 2 red.
Origin: 2 introduced, 0 pre-existing.

- F1, warning, introduced. A diff that is marked neither `truncated` nor `binary` must be one
  `git apply` applies. For a path with a component Git reserves for itself, Git refuses the
  diff by its own path rule. Closed on the branch: the limit is stated in
  `docs/design/01-contract.md` § 3.1, in the doc comment of `bounded_diff` and in the changelog,
  and the case holds it for seven paths, each also applied under a neutral name.
- F2, warning, introduced. The cleanup of a failed replacement must not cost a second request
  its write. Closed on the branch: a replacement with `create_parents` whose write finds its
  directory absent runs once more from its precondition; one retry is the bound. Through the
  daemon two changes of one workspace do not reach the driver together (`lock_workspace`).

Checked and not faulted: 400 generated before and after pairs and 272 end-of-file pairs applied
by `git apply` byte for byte; the 200-line bound and the cut marker; 20,000 generated line
patches against a model; 40,000 generated text edits against a brute-force count; a link put
under a created name; removal through held directory descriptors; no existing case weakened.

Not checked by the reviewer: `patch -p1`; the daemon's `http` target; the result bound at the
ledger's reserve through the daemon; a race of two threads; quota and sync failures; mutants on
a built copy.

