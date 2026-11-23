---
format: aep.planning-md/3
id: story:file-change-diff-is-a-unified-diff-git-applies
kind: story
status: draft
title: The diff of a file change is a unified diff that Git applies
summary: The diff has a hunk header without line ranges and a blank line between removal and addition; git apply rejects it
tags:
- defect
revision: 1
---
# Story: the diff of a file change is a unified diff that Git applies

## Outcome

The `diff.text` of a v2 file change that is not truncated and not binary applies with
`git apply` to the file as it was before and gives the file as it is after.

## What was seen

Release 0.24.0: changing one line from `alpha` to `bravo` returned a diff marked complete whose
hunk header was `@@` without ranges, with a blank line between the removed and the added line.
`git apply` exited 128 on it and 0 on a standard unified diff of the same change. The file held
the right bytes. `bounded_diff` in `crates/substrate-guarded/src/fs.rs` writes the header and
joins lines that still carry their newline.

## What changes

`bounded_diff` writes hunks with line ranges, the `\ No newline at end of file` marker where it
applies, and no line of its own between lines. A truncated diff says so and is not claimed to
apply. The byte bound of
`story:file-change-result-fits-the-ledger-before-the-file-changes` stays.

## Design

The response schema calls the member a unified diff and does not change. Before code: the
released vectors and the frozen bundles are read for a response that carries today's text. If
one does, this story needs a successor bundle and says so.

## Acceptance

A case that applies the returned diff with `git apply` in a temporary repository for a
replacement, an edit and a patch, including a file without a final newline and a new file. Red
before the change.

## Order

After `story:file-change-result-fits-the-ledger-before-the-file-changes`.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 5, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
