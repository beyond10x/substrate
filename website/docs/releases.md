---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

The newest release is 0.26.0, published on 23 November 2026. The
[status page](./status.md#newest-release) summarizes it.

A release that moves the contract bundle needs its SDK clients upgraded with the daemon. See the
[deployment rules](./guides/deployment.md#rules-that-do-not-change-by-posture).

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.26.0 (2026-11-23)

### Added in 0.26.0

- The root of a workspace is the directory `.` on the v1 directory page (ADR 0052).
  `GET /v1/workspaces/{workspace_id}/files/.?mode=directory` lists the root's immediate children
  in name order with the page's `cursor` and `limit_items`, and its result's `path` is `"."`. A
  root with more children than one page is listed completely by following the cursor. ADR 0043
  holds there as in a named directory: an entry the wire cannot name is counted in
  `x-b10x-omitted-entries` on every page, `.git` is neither listed nor counted, and a symbolic
  link is listed as a link. `.` is admitted on that page only: a read in file mode, a write, a
  delete and every v2 file route still refuse it `workspace.path-escape` (422), and the directory
  page still refuses `a/.`, `./a` and `..`. The namespace driver's workspace keeper serves the
  same page. No bundle changes: the request and the result validate against the schemas of
  `substrate-wire/0.27.0`. A daemon or keeper older than this entry answers `/files/.` with
  `workspace.path-escape`, which a client reads as "no root page". In the Rust SDK
  `Workspace::read_directory` and `Workspace::read_directory_with_omissions` take `"."`, and
  `substrate_wire::validate_directory_address` is the rule.

### Fixed in 0.26.0

Four fixes of the v2 changes of one file (design 01, § 3.1, *The v2 changes of one file*). No
route, schema or refusal code changes.

- A line patch (`workspace.file.patch-v2`) changes only the lines it names. A line no edit
  names keeps its bytes, its LF or CRLF included; a line an edit writes ends as the line the
  edit names ends; and a file whose every line is deleted is empty. One case is as before:
  deleting a last line that has no line ending leaves the file without one.
- A text edit (`workspace.file.edit-v2`) is refused `workspace.edit-ambiguous` when `old_text`
  starts at more than one offset of the file, also when the occurrences overlap, under both
  match policies: `ana` in `banana` is refused.
- The `diff.text` of a v2 file change is a unified diff that `git apply` applies to the former
  file, when it is not empty, not `truncated` and not `binary`: one hunk with both line ranges
  in its header, at most three unchanged lines on either side of the changed ones,
  `\ No newline at end of file` where a last line has no ending, and `/dev/null` as the former
  file of a file the change creates. A change without a changed line carries an empty text. This
  holds for a path Git admits in a working tree. For a path Git refuses by its own path rule (a
  component it reads as its `.git` directory, such as `.GIT`, `git~1` or `.git.`) the text has
  the same form and `git apply` refuses it by name (`invalid path`). Two changes far apart in
  one file are one hunk, which is `truncated` over 200 lines.
- A v2 replacement with `create_parents` whose write fails removes the directories it created,
  deepest first; a directory that existed before, or that holds an entry the request did not
  create, stays. A replacement with `create_parents` whose write finds the file's directory
  removed by another writer runs once more from its precondition and creates the parents
  again; a second failure is the answer.
