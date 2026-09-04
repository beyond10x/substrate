---
format: aep.planning-md/1
id: story:unattached-claimed-session-is-contained
kind: story
status: active
title: A session whose attachment claim never upgraded is terminated rather than left running
summary: The claim is consumed before the WebSocket upgrade; a dropped client leaves the process running until lease or timeout (sessions.rs attach).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
- informed_by: story:upgraded-connections-keep-their-permit
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
revision: 5
---
# Story: A claimed but unattached session is contained

## Context

`pipe_session_attach` consumes the durable attachment claim before the WebSocket upgrade completes
(`crates/substrate-daemon/src/app/sessions.rs`, `claim_pipe_session_attachment` then
`ws.on_upgrade`). If the client drops between the `101` and the upgrade, the `on_upgrade` closure
never runs, the permit is dropped, the claim stays `attached`, and the process runs unattached
until its lease or timeout ends it. No test covers this path.

## Acceptance

A client that drops after the attachment claim and before the upgrade leaves no running process
within one maintenance tick, and the session reads as terminal with a named refusal; a test proves
it.

## Notes

Either terminate the session when the upgrade future fails, or claim inside the upgrade task and refuse the upgrade when the claim fails. This is the same hyper upgrade hand-off seam story:upgraded-connections-keep-their-permit observes first (`informed_by`); read its observation before choosing, and if that story moves the permit into the upgrade task, put the claim there too so the two changes are one edit to `on_upgrade`.
