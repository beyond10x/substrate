---
format: aep.planning-md/3
id: story:failed-file-replacement-removes-the-directories-it-created
kind: story
status: draft
title: A file replacement that fails removes the directories it created
tags:
- defect
scope:
- confidence: cited
  path: crates/substrate-daemon/tests/http
- confidence: cited
  path: crates/substrate-guarded/src/fs.rs
revision: 2
---
# Story: a file replacement that fails removes the directories it created

## Outcome

A v2 file replacement with `create_parents` whose write fails leaves the workspace as it was:
the parent directories this call created are removed again.

## The rule

A replacement refused for its text bounds, its digest, its content, its size or its
precondition creates no directory (release `0.25.0`). This story extends the rule to a write
that fails after the directories exist: a file name the file system refuses, a quota, a
failed write, sync or rename of the temporary file.

## What changes

`replace_cas` in `crates/substrate-guarded/src/fs.rs` records which directories
`create_parent_directories` made in this call and removes those, deepest first, when
`write_atomic` fails. A directory that existed before the call, or that another request has
put an entry into since, stays.

## Design

No contract changes: the refusal codes are the published ones.

## Acceptance

A daemon case: a replacement with `create_parents` whose file name is 256 bytes long is
answered with the published refusal and the workspace's listing is what it was before. A
second case with the over-long name as the last parent. Both under 5 s.

