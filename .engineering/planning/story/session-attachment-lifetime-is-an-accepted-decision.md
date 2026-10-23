---
format: aep.planning-md/3
id: story:session-attachment-lifetime-is-an-accepted-decision
kind: story
status: implemented
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
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T08:28:31Z", actor: "agent:b10x-chef-orchestrator", revision: 5, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-23T08:28:31Z", actor: "agent:b10x-chef-orchestrator", revision: 6, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:56Z", actor: "agent:b10x-chef-orchestrator", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
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

ADR 0008, or a later ADR that amends it (and is linked from it), records the one-hour attachment lifetime and the no-re-attach rule together with the consumer they were chosen for.

## Notes

If re-attach is wanted instead, the ADR says so and a story is filed; that is a contract change (a new attachment state and refusal) and needs a successor bundle. `docs/design/05-streams-sessions-and-endpoints.md` and the published session capability document are brought into agreement with whatever the ADR records.
