---
format: aep.planning-md/3
id: story:whitespace-edit-match-is-linear-in-file-size
kind: story
status: draft
title: The whitespace-tolerant edit match costs time linear in the file
summary: Lines are normalized once and matched in one pass with a bounded match count
tags:
- defect
revision: 1
---
# Story: the whitespace-tolerant edit match costs time linear in the file

## Outcome

A valid edit at the schema's bounds does not hold a file-system worker of the daemon for longer
than a stated time. Doubling the input about doubles the work.

## What changes

`apply_text_edit` in `crates/substrate-guarded/src/fs.rs`: each line is normalized once, and the
fallback matcher is a linear sequence match with a bounded number of matches.

## Design

No contract changes: the same edits match.

## Acceptance

- The matcher's existing cases stay green.
- A case counts comparison steps for two input sizes and holds the ratio; it is timed once
  before it is added and its time is named in the pull request.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 10. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
