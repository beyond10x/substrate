---
format: aep.planning-md/1
id: story:directory-listing-survives-special-entries
kind: story
status: draft
title: A FIFO, socket, device or non-UTF-8 name never makes a directory page or tree refuse
summary: list_directory and walk_tree answer workspace.path-escape for special entries a confined process can create (fs.rs:768-813).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/fs.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
revision: 3
---
# Story: Directory listing survives special entries

## Context

`list_directory` and `walk_tree` return `workspace.path-escape` for any entry that is not a
regular file, directory or symlink, and for any name that is not UTF-8
(`crates/substrate-host/src/fs.rs:768`, `:773`, `:803`, `:813`). A confined process can create a
FIFO (`mkfifo`) or a byte-named file in `/workspace`; afterwards every directory page of that
directory and the whole `GET /v2/workspaces/{id}/tree` refuse. The refusal names an escape that did
not happen, and the client cannot list its own workspace. Code reading only; not reproduced at
runtime.

## Acceptance

A directory holding a FIFO, a socket and a non-UTF-8 name lists its regular entries with `200` on
both the v1 directory page and the v2 tree, and a test creates those three entries and reads both.

## Notes

The wire `DirectoryEntryKind` has three members (`crates/substrate-wire/src/lib.rs:1045`). Naming
a fourth is a contract change and needs a successor bundle under invariant 6; skipping such
entries with the existing `truncated` flag on the tree, and omitting them from the page, needs
none. The story body of whichever path is taken records the choice before code (invariant 8).

## Parallel work

This story shares `crates/substrate-host/src/fs.rs` with story:directory-listing-survives-special-entries and story:line-patch-edits-apply-in-line-order; the two touch different functions (`apply_line_patch` versus `list_directory`/`walk_tree`) but land on one file, so they are worked in sequence, not at once.
