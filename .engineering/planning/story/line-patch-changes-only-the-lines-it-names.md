---
format: aep.planning-md/3
id: story:line-patch-changes-only-the-lines-it-names
kind: story
status: draft
title: A line patch changes only the lines it names
summary: A line patch rebuilds the whole file with one newline style and appends the final newline even when no line remains
tags:
- defect
revision: 1
---
# Story: a line patch changes only the lines it names

## Outcome

After a v2 line patch, every byte outside the patched lines is the byte that was there before:
the line endings of untouched lines stay, and a file whose only line is deleted is empty.

## What was seen

Release 0.24.0, read back from the daemon: deleting the only line of a six-byte LF-terminated
file left one newline byte. In a file with mixed line endings, replacing line three changed the
untouched line two from LF to CRLF. The patcher in `crates/substrate-guarded/src/fs.rs` rebuilds
the file with one newline style.

## What changes

The patcher keeps each untouched line with its own ending and writes no final newline that
belongs to a deleted line. What an inserted or replaced line gets as its ending is stated in the
code and in design 22.

## Design

No schema changes. The rule is added to the design of the v2 file mutations in one paragraph.

## Acceptance

Cases in the guarded crate: the two observed files, plus a file without a final newline and a
CRLF-only file; each compares every byte. Red before the change.

## Order

After `story:file-change-result-fits-the-ledger-before-the-file-changes`, which changes the
same file.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 3, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
