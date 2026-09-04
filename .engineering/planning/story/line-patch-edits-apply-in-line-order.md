---
format: aep.planning-md/1
id: story:line-patch-edits-apply-in-line-order
kind: story
status: draft
title: A line patch applies the same result whatever order its edits arrive in
summary: apply_line_patch iterates edits in reverse input order; unsorted edits land on the wrong lines (fs.rs:1226), reproduced at runtime.
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/fs.rs
revision: 3
---
# Story: Line patch edits apply in line order

## Context

`apply_line_patch` validates edit ranges on a sorted copy but applies the edits by iterating the
request in reverse **input** order (`crates/substrate-host/src/fs.rs:1226`). An edit list that is
not ascending shifts later edits onto the wrong lines. Reproduced outside the crate with the
function copied verbatim: on a six-line file, `[replace line 5, insert before line 2]` yields
`L1 A B L2 X L4 L5 L6` while the ascending order of the same edits yields `L1 A B L2 L3 L4 X L6`.
The published schema `contracts/substrate-wire/0.15.0/schemas/inputs/workspace-file-patch-v2.json`
states no ordering rule for `edits`, and no released vector exercises two edits.

## Acceptance

`POST /v2/workspaces/{id}/file-patches/{path}` with the same set of non-overlapping edits produces
byte-identical files for every permutation of the list, proven by a `fs::tests` case that submits
two permutations of a two-edit patch.

## Notes

Smallest fix: sort by `(start_line, kind)` descending before the apply loop. Alternative that
needs a successor bundle: refuse a non-ascending list with a new `workspace.patch-order` code.
Either way the released bytes of `0.15.0` do not change.

## Parallel work

This story shares `crates/substrate-host/src/fs.rs` with story:directory-listing-survives-special-entries and story:line-patch-edits-apply-in-line-order; the two touch different functions (`apply_line_patch` versus `list_directory`/`walk_tree`) but land on one file, so they are worked in sequence, not at once.
