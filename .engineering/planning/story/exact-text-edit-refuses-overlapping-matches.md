---
format: aep.planning-md/3
id: story:exact-text-edit-refuses-overlapping-matches
kind: story
status: draft
title: An exact text edit refuses text found at two overlapping places
summary: The edit counts non-overlapping matches, so "ana" in "banana" counts as one and is replaced
tags:
- defect
revision: 1
---
# Story: an exact text edit refuses text found at two overlapping places

## Outcome

A v2 text edit whose `old_text` occurs at more than one offset of the file is refused with the
existing refusal for a match that is not unique, also when the occurrences overlap.

## What was seen

Release 0.24.0: replacing `ana` with `X` in `banana` returned 200 and wrote `bXna`. The matches
start at offsets 1 and 3. `apply_text_edit` in `crates/substrate-guarded/src/fs.rs` counts
`match_indices`, which skips overlapping matches. The published input schema defines a refusal
for text found at more than one location.

## What changes

The count is of start offsets, overlapping ones included. The whitespace-normalized match gets
the same reading. The count stays linear in the file size
(`story:whitespace-edit-match-is-linear-in-file-size`).

## Design

No schema changes: the refusal exists.

## Acceptance

Cases in the guarded crate: `ana` in `banana`, `aa` in `aaa`, and a normalized match that
overlaps; each is refused and the file is unchanged. Red before the change.

## Order

After `story:file-change-result-fits-the-ledger-before-the-file-changes`, which changes the
same file.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 4, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
