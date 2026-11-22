---
format: aep.planning-md/3
id: story:git-read-limits-bound-the-work
kind: story
status: draft
title: A Git read limit bounds the work and not only the answer
summary: Baseline size is refused from the object header; enumeration and patch rendering stop at their limits
tags:
- defect
relations:
- depends_on: story:git-changes-reader-reads-only-the-admitted-workspace
revision: 1
---
# Story: a Git read limit bounds the work and not only the answer

## Outcome

`max_bytes`, `max_files` and the response byte budget of the Git observation routes bound what
the daemon reads, hashes and renders. A small budget on a repository with large objects or many
changes costs a small amount of memory and time.

## What changes

1. The baseline reader refuses an object over `max_bytes` from its header, before it loads it.
2. The changes reader stops enumerating at `max_files` and stops rendering at the byte budget.

## Design

No contract changes: the same refusals and truncation marks, reached earlier.

## Acceptance

Unit cases in `crates/substrate-host/src/git.rs`: an object over the bound is refused without
being loaded, and the reader opens no more files than the limit; each red before the change.

## Order

After `story:git-changes-reader-reads-only-the-admitted-workspace`, which replaces the reader
part 2 changes.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 11. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
