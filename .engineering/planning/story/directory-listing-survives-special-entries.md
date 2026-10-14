---
format: aep.planning-md/3
id: story:directory-listing-survives-special-entries
kind: story
status: active
title: A FIFO, socket or non-UTF-8 name never makes a directory page or tree refuse
summary: list_directory and walk_tree answer workspace.path-escape for special entries a confined process can create (fs.rs:768-813).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/fs.rs
- confidence: cited
  path: website/docs/reference/contract.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":8}}}
- {from: "proposed", to: "active", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":8}}}
---
# Story: Directory listing survives special entries

## Context

`list_directory` and `walk_tree` return `workspace.path-escape` for any entry that is not a
regular file, directory or symlink, and for any name that is not UTF-8
(`crates/substrate-host/src/fs.rs:764-840`). A confined process can create a FIFO (`mkfifo`), a
socket or a byte-named file in `/workspace`; afterwards every directory page of that directory and
the whole `GET /v2/workspaces/{id}/tree` refuse. The refusal names an escape that did not happen,
and the client cannot list its own workspace. Code reading only; not reproduced at runtime.

Block and character devices fall into the same `_ =>` arm (`fs.rs:776`, `:819`) and are covered by
the same change. A test cannot create one unprivileged (`mknod` needs `CAP_MKNOD`), so the tests
create a FIFO, a socket and a non-UTF-8 name, and the device case is covered by the shared branch.

## Decision (recorded before code, invariant 8)

No wire change. The wire `DirectoryEntryKind` keeps its three members
(`crates/substrate-wire/src/lib.rs:1234`) and `DirectoryPage` keeps its shape (`:1038`); naming a
fourth kind or adding a page field would need a successor bundle under invariant 6, and this story
does not cut one.

- An entry whose type is not file, directory or symlink, or whose name is not UTF-8, is **omitted**
  from the v1 directory page and from the v2 tree, as `.git` already is.
- On the v2 tree, an omission sets the existing `truncated` flag to `true`, so a client is told the
  answer is incomplete. `website/docs/reference/contract.md` states, beside the workspace routes,
  that both routes omit such entries and that `truncated` also reports an omission.
- Omitted entries are never descended into and never counted toward the tree's entry limit.
- `workspace.path-escape` stays the answer for a path that really escapes; no other refusal changes.

**Accepted trade-offs against invariant 3**, recorded rather than hidden:

1. The v1 page omits an entry with no machine-readable signal; the documentation sentence is its
   only statement. The alternative, today's refusal, makes the whole directory unlistable and names
   an escape that did not happen, which is worse for the same invariant.
2. On v2, `truncated` now has two causes: the entry limit (`fs.rs:806`) and an omission. A client
   that retries with a higher `limit_items` after an omission gets `truncated: true` again. The
   documentation names both causes. The published schema describes `truncated` as a bare boolean
   (`contracts/substrate-wire/0.18.0/schemas/results/workspace-tree-read-v2.json`), so no released
   meaning changes.

A distinct machine-readable signal on both routes (an omitted count, or a fourth entry kind) needs a
successor bundle and is story:directory-listing-names-omitted-entries, a draft outside this wave.

## Acceptance

A directory holding a FIFO, a socket and a non-UTF-8 name lists its regular entries with `200` on
both the v1 directory page and the v2 tree, the tree answers `truncated: true`, a test creates those
three entries and reads both routes, and `website/docs/reference/contract.md` states the omission on
both routes and the second meaning of `truncated`.

## Parallel work

Lands on `crates/substrate-host/src/fs.rs` (`list_directory`, `walk_tree`), its tests and
`website/docs/reference/contract.md`. No other unit of this wave touches those files. The
`CHANGELOG.md` entry is written by the integrator at integration, not by the unit, so it does not
collide with the session-containment unit's entry.
