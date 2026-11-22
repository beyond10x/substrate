---
format: aep.planning-md/3
id: story:file-change-result-fits-the-ledger-before-the-file-changes
kind: story
status: draft
title: A file change is written only when its result fits the ledger
summary: The result of a v2 file change is bounded in serialized bytes before the file is written, and the server holds the schema's text bounds
tags:
- defect
- security
revision: 1
---
# Story: a file change is written only when its result fits the ledger

## Outcome

Every admitted v2 replacement, edit and patch ends with a successful, replayable terminal
record. No request changes a file and then reports a failure for it.

## What changes

1. `crates/substrate-guarded/src/fs.rs`: the result's text is bounded by serialized bytes, JSON
   escaping counted, to fit the terminal-result budget of the ledger, and the bound is computed
   before the file is written.
2. The result obeys the `maxLength` its own schema publishes.
3. The server refuses `old_text` and `new_text` over the schema's `maxLength`, by name. Today
   only a client that validates holds that bound.
4. The other success payloads are read for the same budget, and the result is written here.

## Design

No wire schema changes: the bounds are the published ones. A result that is shorter than today
is within the schema.

## Acceptance

- Cases in the guarded crate: a text with long lines and one with characters that JSON escapes,
  each within the file limit, change the file and leave a completed row; red before the change.
- A request over the text bound is refused before anything is written.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 5. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
