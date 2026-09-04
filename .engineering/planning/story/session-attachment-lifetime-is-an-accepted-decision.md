---
format: aep.planning-md/1
id: story:session-attachment-lifetime-is-an-accepted-decision
kind: story
status: draft
title: The 1 h attachment lifetime and kill-on-disconnect are a recorded decision
summary: Attachments live 1 h, any disconnect kills the tree, no re-attach exists (sessions.rs:70,1301); the figure is undocumented.
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: adr/0008-pipe-sessions-have-distinct-durable-identity.md
- confidence: cited
  path: docs/design/05-streams-sessions-and-endpoints.md
revision: 4
---
# Story: The session attachment lifetime is an accepted decision

## Context

A session attachment lives at most one hour (`crates/substrate-daemon/src/app/sessions.rs:70`);
when that expires, or on any disconnect or protocol error, the process tree is killed
(`sessions.rs:1301`) and the one-shot attachment right is consumed
(`crates/substrate-store/src/sessions.rs`, `claim_pipe_session_attachment_inner`). No re-attach
exists. ADR 0008 states the kill-on-loss rule; nothing states the one-hour figure or that an
interactive agent session longer than an hour is out of scope.

## Acceptance

ADR 0008, or a successor ADR that supersedes it, records the one-hour attachment lifetime and the no-re-attach rule together with the consumer they were chosen for.

## Notes

If re-attach is wanted instead, the ADR says so and a story is filed; that is a contract change (a new attachment state and refusal) and needs a successor bundle. `docs/design/05-streams-sessions-and-endpoints.md` and the published session capability document are brought into agreement with whatever the ADR records.
