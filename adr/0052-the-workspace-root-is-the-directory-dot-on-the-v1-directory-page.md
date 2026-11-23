---
status: accepted
date: 2026-11-23
---

# ADR 0052: the workspace root is the directory `.` on the v1 directory page

Decider: the operator, 2026-11-23
(`approval-record:operator-picks-dot-as-the-root-of-the-directory-page-2026-11-23`). Decides
`story:root-directory-listing-pages-immediate-children`. The design comes before code
(invariant 8): this record changes what one published route answers and no code yet.

## Context

A client cannot page the immediate children of a workspace's root.

The v1 directory page, `GET /v1/workspaces/{workspace_id}/files/{*path}?mode=directory`, lists one
directory in name order with `cursor` and `limit_items` (at most `MAX_LIST_ITEMS`, 1,000). Its
path is validated by `validate_relative_path` in `crates/substrate-wire/src/lib.rs`, which refuses
the empty path and every `.` component. `/files/.` is therefore 422 `workspace.path-escape`, and
`/files/` and `/files` match no route and are 404 `resource.not-found`.

For the root a client falls back to the v2 tree, `GET /v2/workspaces/{workspace_id}/tree`. The tree
walks depth-first in name order, stops at `limit_items` and has no depth, prefix or cursor. Its
`truncated` promises only that the tree is incomplete ([ADR 0043](0043-a-listing-names-the-entries-it-omits-in-a-response-header.md)
decision 5). A first subdirectory with 1,000 descendants hides every later child of the root.
Observed on release `0.24.0`: a workspace with one top-level directory holding 1,000 files
answered 1,000 entries and `truncated`. The workspace service answers 503 when it reads that flag
for a root.

The published path schema, `relative-path` in `schemas/common.json` of bundle `0.27.0`, has
`minLength` 1 and a pattern that admits `.`. The reader is stricter than the schema.

## Decision

1. **The root is the path `.` on the v1 directory page.**
   `GET /v1/workspaces/{workspace_id}/files/.?mode=directory` lists the immediate children of the
   workspace's root with the paging of every other directory page. The result's `path` is `"."`.
2. **`.` is admitted there and nowhere else.** A read in file mode, a write, a delete and every
   v2 file route refuse `.` as before, 422 `workspace.path-escape`. So do `a/.`, `./a` and `..`
   on the directory page. The empty path stays unrouted.
3. **Nothing else about the page changes.** Name order, the cursor and its refusal
   (`workspace.cursor-invalid`), the limit, and ADR 0043's rules hold at the root: an entry the
   wire cannot name is omitted and counted in `x-b10x-omitted-entries` on every page, and `.git`
   is neither listed nor counted. A symbolic link at the root is listed as a link and not
   followed.
4. **No successor bundle.** Every request and response of decision 1 validates against the
   schemas bundle `0.27.0` publishes: `.` satisfies `relative-path` as an address and as the
   result's `path`. No route, schema, header or refusal code is added.
5. **A machine that does not serve it says so by name.** A daemon, or a workspace keeper of the
   namespace driver, older than this decision answers `/files/.` with 422
   `workspace.path-escape`. A client reads that code as "this machine has no root page" and may
   fall back to the tree.
6. **The next successor bundle publishes it.** Whatever cuts the next bundle adds a vector for
   the root page and states decision 2 in the path schema's description. Until then the rule
   lives in this record, the wire crate and the runtime vectors.

## Alternatives

- **The empty path, `/files/`.** The published `relative-path` has `minLength` 1, so the address
  and the result's `path` would widen: successor bundle `0.28.0`. The Rust SDK pins the bundle by
  name and digest, so every client would be rebuilt and released with the daemon. Refused for
  that cost: the same listing is reachable inside the published schemas.
- **A depth and a cursor on the v2 tree.** The tree's result is a closed object, so a resume
  point needs a header or a new result, and its items carry `path` where a directory page
  carries `name`. A second paging model beside the directory page, and a successor bundle.
- **A new route for the root.** One more route, three schemas and an unserved refusal for one
  directory the existing page can list.

## Consequences

- Two daemons that advertise `substrate-wire/0.27.0` answer `/files/.` differently until the
  next bundle: one lists the root, an older one refuses by name. Release `0.10.0` changed what
  the listings answer inside the schemas in the same way, and bundle `0.23.0` published it later
  (ADR 0043). Invariant 6 is kept in its letter: no released bundle directory changes.
- A client that removes dot segments from a URL before it sends it turns `/files/.` into
  `/files/` and gets 404. The Rust SDK does not: it encodes the path itself. The public contract
  page says that the segment is sent as written.
- The page rescans the directory for every page and its cursor binds no snapshot, as on every
  directory page. A root that changes between pages can repeat or miss an entry. Unchanged.

## What the implementation owes

- `crates/substrate-wire`: a validator for a directory address that admits exactly `.` beside
  every relative path.
- The daemon's file read and `crates/substrate-guarded`: `.` in directory mode opens the
  workspace's own directory. The namespace driver forwards the same read to its keeper.
- `crates/b10x-substrate-sdk`: `read_directory` and `read_directory_with_omissions` admit `.`.
- Runtime vectors, on the portable, delegated and namespace lanes: a root with more children
  than two pages listed completely by following the cursor, in name order, without a duplicate;
  a FIFO at the root counted on every page; a symbolic link at the root listed and not
  followed; `.git` neither listed nor counted; an empty root; `.` refused in file mode, on
  write, on delete and on the v2 file routes; `a/.` and `./a` refused on the directory page; a
  cursor past the end refused.
- `website/docs/reference/contract.md` states the root page.
