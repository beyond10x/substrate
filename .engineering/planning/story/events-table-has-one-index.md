---
format: aep.planning-md/3
id: story:events-table-has-one-index
kind: story
status: implemented
title: The events table carries no index duplicating its primary key
summary: events_subject_sequence duplicates the WITHOUT ROWID primary key (schema.rs:203-206).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-store/src/schema.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T09:53:42Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-14T09:53:43Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-14T17:00:32Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# Story: The events table has one index

## Context

`events` is a `WITHOUT ROWID` table with primary key `(deployment, subject, seq)`; the schema also
creates `events_subject_sequence` on the same three columns
(`crates/substrate-store/src/schema.rs:203-206`). Every event append writes two identical B-trees,
and the migration at `schema.rs:303` recreates the duplicate.

## Acceptance

`PRAGMA index_list(events)` shows no index whose columns equal the primary key, on a fresh store
and on a store migrated from the current schema, proven by a store test.

## Notes

`DROP INDEX IF EXISTS events_subject_sequence` in a migration step; the primary key serves the
same query.
