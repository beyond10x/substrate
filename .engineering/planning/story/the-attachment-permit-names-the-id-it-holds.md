---
format: aep.planning-md/1
id: story:the-attachment-permit-names-the-id-it-holds
kind: story
status: draft
title: The attachment permit names the id it holds
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
revision: 2
---
# Story: The attachment permit names the id it holds

## Context

`crates/substrate-daemon/src/app/sessions.rs:91` — `PipeAttachmentPermit.exec_id` — and
`PipeAttachmentLimits::acquire(scope, exec_id)` are both passed a **session** id: `sessions.rs:1170`
passes `&session_id`. In the same file, `terminate_pipe_session(app, scope, exec_id)` takes a real
exec id.

Nothing breaks: the insert and the `Drop` remove use the same key, whatever it is called. Found
during the 2026-09-04 security wave (`review-result:adversary-u3-pass-2`), three lines from the
permit machinery that wave changed, which is exactly where a reader will be when the name misleads
them.

## Acceptance

Every parameter and field in the attachment-permit machinery is named for the kind of id it holds,
and no session id is passed to a parameter named `exec_id`.

## Notes

Rename only; no behaviour changes and no case should need to move. If the two kinds of id are the
same type today, consider whether they should be — a newtype makes this class of mistake a compile
error rather than a reading exercise.
