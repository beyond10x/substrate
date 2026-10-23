---
status: accepted
date: 2026-10-23
---

# ADR 0043: a listing names the entries it omits in a response header

Decides `story:directory-listing-names-omitted-entries`. It adds one response header to two
existing routes, keeps what the v2 tree's `truncated` answers, and publishes the header in the
unreleased bundle `0.23.0`.

## Context

Since `story:directory-listing-survives-special-entries` (release `0.10.0`), the v1 directory page
(`GET /v1/workspaces/{workspace_id}/files/{*path}?mode=directory`) and the v2 tree
(`GET /v2/workspaces/{workspace_id}/tree`) omit every entry the wire cannot name instead of
refusing: an entry that is not a regular file, directory or symbolic link (a FIFO, socket or
device), and an entry whose name is not UTF-8 (`representable_entry` in
`crates/substrate-host/src/fs.rs`). Without a successor bundle that change could signal the
omission in only one place: the tree reused `truncated`, which then meant either "the walk reached
`limit_items`" or "an entry was omitted", and the v1 page signalled nothing. Both were recorded as
trade-offs against invariant 3, which says a missing guarantee is named, never silent.

Reusing `truncated` costs the client its one way to tell the two causes apart. A client that sees
`truncated: true` and retries with a higher limit, as it should after reaching the limit, gets
`truncated: true` again forever when the cause was a FIFO.

Narrowing `truncated` to the limit alone would fix that, but it would break a consumer that reads
`truncated` as "this tree is incomplete": the workspace service
(`workspace/crates/workspace-service/src/main.rs`, its legacy coding-tree projection and its
root-page projection) calls the SDK's `Workspace::tree` and takes `!tree.truncated` as proof that
the tree is complete. If an omission stopped setting `truncated`, that service would present a tree
missing a FIFO as complete. Breaking a consumer's documented reading is not this repository's call
to make.

Both result objects are closed: `DirectoryPage` and `WorkspaceTree` are
`#[serde(deny_unknown_fields)]` in `crates/substrate-wire/src/lib.rs`, and their bundle schemas set
`additionalProperties: false`. A new body member would make every client pinned to an earlier wire
crate or bundle reject every listing, including the ones that omitted nothing.

## Decision

1. **The omission signal is the response header `x-b10x-omitted-entries`.** Its value is the
   number of entries the listing omitted, written as a decimal integer of at least 1 with no sign
   and no leading zero. The header is present on a successful v1 directory page and on a
   successful v2 tree exactly when that count is at least 1, and absent otherwise. No body member
   changes, so a listing that omits nothing is byte-identical to before and one that omits
   something still validates against every earlier bundle.
2. **What is counted is what the wire cannot name.** An entry counts when it is listed in the
   directory being read and `representable_entry` refuses it. `.git` is never counted: it is left
   out by policy, not because the wire cannot name it. On the v2 tree a hidden entry left out
   because `include_hidden` is false is not counted either; with `include_hidden` true, a hidden
   unrepresentable entry counts like any other. An omitted directory (a name that is not UTF-8)
   counts once, and the walk does not descend into it.
3. **On the v1 page the count is the directory's, not the page's.** Omitted entries are on no page,
   so every page of one directory carries the same count, and a client sees the omission on the
   first page it reads.
4. **On the v2 tree the count covers the part of the tree that was walked.** The walk counts an
   omission before it checks the limit, so an omitted entry never consumes `limit_items`. When the
   walk stops at the limit, entries it did not reach are not counted, so the count is a lower bound
   exactly when the walk reached `limit_items` with entries left, and exact otherwise.
5. **`truncated` keeps its `0.10.0` meaning, "this tree is incomplete".** The tree answers
   `truncated: true` when the walk reached `limit_items` with entries left, or when it omitted at
   least one entry, exactly as since `0.10.0`, so a client that reads it as "incomplete" stays
   correct. The header is the new signal and is distinct from `truncated`: a client tells the two
   causes apart by reading both. `truncated: true` without the header means the limit only;
   `truncated: true` with the header means at least one omission, and possibly also the limit.
6. **The header name carries the `x-b10x-` prefix of the existing contract headers**
   (`x-b10x-contract`, `x-b10x-contract-bundle-sha256`). It is a wire-visible identifier from the
   bundle that publishes it on, frozen by invariant 6 like the others.
7. **Bundle `0.23.0` publishes the header** on the directory result of `workspace.file.read` and on
   the result of `workspace.tree.read-v2`, as an `x-b10x-response-headers` annotation naming the
   header, its value form, what its absence means and that it is distinct from `truncated`. The
   tree's `truncated` member states both causes and that the header tells them apart.
   `compatibility.json` states the change.
8. **The Rust SDK reads the header without changing a signature.** `Workspace::tree` and
   `Workspace::read_directory` keep their signatures and results. Two new methods,
   `tree_with_omissions` and `read_directory_with_omissions`, answer the same result together with
   the omitted count, zero when the header is absent. The header name is the `substrate_wire::OMITTED_ENTRIES_HEADER` constant; the daemon
   writes that constant and `cargo xtask check-bundle` holds the annotation to it from
   `xtask/src/bundle/omitted_entries.rs`, because `xtask/src/render.rs` cannot gain a binding.

## Consequences

- A client can tell "raise the limit" from "the directory holds entries I cannot see", on both
  routes, without decoding a new body member.
- No client sees a change it did not ask for: the bodies are the same objects with the same
  values, `truncated` answers as in `0.10.0`, and a client that ignores unknown response headers
  ignores this one. The workspace service keeps treating a tree with an omission as incomplete.
- The retry problem stays for a client that reads only `truncated`: after an omission, raising the
  limit still answers `truncated: true`. Such a client stops retrying by reading the header; a
  later decision may narrow `truncated` once its consumers read the header, and that decision is a
  coordinated change with them.
- The count is an observation at read time, like the listing it accompanies. A v1 client that
  pages a directory while its entries change can see different counts on different pages.
- The `Driver` contract changes: `read_workspace_path` and `list_workspace_tree_v2` return the
  listing together with its omission count (`substrate_host::Listed`), so a driver states the count
  rather than a default inventing zero for it (invariant 3).
- The omitted entries stay addressable by path exactly as before; how a file read of such a path
  is answered is unchanged by this decision.
