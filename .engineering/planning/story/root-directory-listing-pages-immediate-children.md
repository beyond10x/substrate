---
format: aep.planning-md/3
id: story:root-directory-listing-pages-immediate-children
kind: story
status: draft
title: The root of a workspace is listed in pages of its immediate children
summary: The directory reader needs a non-empty path, so a client lists the root through the recursive tree, which truncates at 1,000 entries
tags:
- defect
revision: 1
---
# Story: the root of a workspace is listed in pages of its immediate children

## Outcome

A client lists the immediate children of a workspace's root with the cursor paging the
directory route gives every named directory.

## What was seen

Release 0.24.0: a workspace with one top-level directory holding 1,000 files answered the
recursive tree with 1,000 entries and `truncated`. The directory reader requires a non-empty
relative path (`crates/substrate-guarded/src/fs.rs`, the directory listing; the SDK at
`crates/b10x-substrate-sdk/src/lib.rs`), so a consumer has no paged listing of the root. The
consumer Workspace answers 503 `coding_tree_root_incomplete` when it sees the flag (read in its
source, not run).

## Design

This is a contract change (invariant 8): a design note comes before code. It decides how the
root is addressed on the directory route, whether the published path schema admits it, and
whether that needs a successor bundle.

## Acceptance

Named by the design note. At least: a root with more immediate children than one page is listed
completely by following the cursor, in a runtime vector.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 2, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Consumer: the repository
`workspace`. Stakeholder: the operator.
